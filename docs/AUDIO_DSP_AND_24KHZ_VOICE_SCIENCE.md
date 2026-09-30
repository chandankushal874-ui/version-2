# Mathematical and Physical Principles of the Real-Time Translation Audio Pipeline

## Abstract
This document provides an in-depth mathematical, physical, and architectural analysis of the digital signal processing (DSP) pipeline in Ollalink Translate. It explains why multilingual voice synthesis (specifically 24,000 Hz neural models for Spanish, French, German, Chinese, Arabic, Portuguese, and Russian) previously exhibited the "chipmunk effect," digital clipping distortion, and mid-sentence cutouts, and details the exact mathematical formulas, interpolation theory, and buffer dynamics that resolved these issues.

---

## 1. The Physics and Mathematics of the "Chipmunk Effect"

### 1.1 Digital-to-Analog Conversion (DAC) Sampling Mechanics
A digital audio signal is a discrete sequence of samples $\{x[n]\}_{n=0}^{N-1}$ obtained by sampling a continuous acoustic pressure wave $x(t)$ at a uniform sampling frequency $F_s$:
$$x[n] = x(n \cdot T_s) = x\left(\frac{n}{F_s}\right)$$
where $T_s = \frac{1}{F_s}$ is the sampling period.

When a digital sound card (DAC) outputs audio on Windows via the WASAPI (Windows Audio Session API) subsystem, its physical clock advances at a fixed native hardware rate $F_{s, \text{DAC}}$, typically:
$$F_{s, \text{DAC}} = 48,000 \text{ Hz}$$
Every second, the DAC hardware demands and consumes exactly 48,000 sample points from the audio output ring buffer.

### 1.2 The Sample Rate Mismatch Equation
If an upstream neural text-to-speech (TTS) engine generates audio at a native rate $F_{s, \text{source}} = 24,000 \text{ Hz}$, each second of speech contains $N = 24,000$ discrete samples.

If the receiving system misdetects or defaults the rate to $F_{s, \text{assumed}} = 48,000 \text{ Hz}$, it passes these 24,000 samples directly to the 48,000 Hz DAC ring buffer without sample rate conversion.

The total physical time $T_{\text{playback}}$ required for the DAC to consume these $N$ samples is:
$$T_{\text{playback}} = \frac{N}{F_{s, \text{DAC}}} = \frac{24,000}{48,000} = 0.50 \text{ seconds}$$

Because the acoustic utterance that took $1.0\text{ second}$ to speak is reproduced in $0.5\text{ seconds}$, the signal undergoes time compression by a scaling factor:
$$\alpha = \frac{F_{s, \text{source}}}{F_{s, \text{DAC}}} = \frac{24,000}{48,000} = \frac{1}{2}$$

### 1.3 Frequency-Domain Transformation (Pitch Doubling)
By the Fourier Transform scaling property:
$$\mathcal{F}\{x(a \cdot t)\} = \frac{1}{|a|} X\left(\frac{\omega}{a}\right)$$
For time scaling $a = 2$ (playback at $2\times$ speed):
$$x_{\text{out}}(t) = x(2t) \implies X_{\text{out}}(\omega) = \frac{1}{2} X\left(\frac{\omega}{2}\right)$$

Every spectral component $\omega_k$ in the human vocal tract (both the fundamental pitch $F_0$ and the formants $F_1, F_2, F_3$) is shifted upwards by a factor of 2:
$$f_{\text{output}} = 2 \cdot f_{\text{input}}$$

In musical acoustics, the interval in semitones $\Delta \text{cents}$ between two frequencies $f_1$ and $f_2$ is:
$$\Delta \text{cents} = 1200 \log_2\left(\frac{f_2}{f_1}\right) = 1200 \log_2(2) = 1200 \text{ cents} = 1 \text{ full octave (+12 semitones)}$$

A male voice with a fundamental frequency of $F_0 = 120\text{ Hz}$ was reproduced at $240\text{ Hz}$ (a high soprano register). Simultaneously, the formant frequencies that characterize vowels ($F_1, F_2$) doubled, shrinking the perceived acoustic vocal tract length to that of a cartoon creature: the classic **"chipmunk effect."**

### 1.4 Why English/Hindi Worked While Other Languages Failed
* **Streaming Lane:** English (`en`) and Hindi (`hi`) routed through the `nh-m01` / `nh-f01` production voices utilize Ollalink's real-time streaming lane, which natively synthesizes and streams raw **48,000 Hz** mono 16-bit PCM.
* **Multilingual Fallback Lane:** Spanish (`es`), French (`fr`), German (`de`), Chinese (`zh`), Arabic (`ar`), Portuguese (`pt`), and Russian (`ru`) route through multilingual neural synthesis engines optimized for batch inference. These models synthesize speech natively at **24,000 Hz**.
* **The Omission Trap:** When Ollalink emitted audio for multilingual languages without explicitly populating the `sample_rate` key in its JSON header frame, the fallback chain defaulted to $48,000\text{ Hz}$.

---

## 2. The Resampling Solution: Continuous Spline Interpolation

### 2.1 The Failure of Block-Based Resamplers in Streaming
Standard resamplers (such as fixed-block FFT sinc resamplers like `FftFixedIn`) require a fixed chunk size (e.g., 1024 input samples) to compute forward and inverse Fourier transforms:
$$X[k] = \sum_{n=0}^{M-1} x[n] e^{-j 2\pi k n / M}$$

In a live WebSocket streaming pipeline, packets arrive in non-uniform fragment sizes (e.g., 160 samples, 320 samples, 800 samples). A block-based resampler must buffer incoming samples until the full block size $M$ is satisfied:
1. **Trapped Sample Latency:** If an utterance ends with 250 samples, they remain trapped inside the resampler's internal buffer until flushed, truncating word endings.
2. **Burstiness:** The resampler outputs nothing while collecting samples, then outputs a large block of samples instantaneously, producing a stuttering playback rhythm.

### 2.2 Continuous 4-Point Catmull-Rom Cubic Spline Interpolation
To achieve zero-latency, sample-by-sample continuous resampling from $24,000\text{ Hz} \to 48,000\text{ Hz}$ (and hot-swappable $44,100\text{ Hz}$ or $96,000\text{ Hz}$), the pipeline implements a continuous Catmull-Rom cubic spline interpolator.

Given four consecutive discrete samples $p_0, p_1, p_2, p_3$ at grid positions $-1, 0, 1, 2$, the continuous signal $p(t)$ for $t \in [0, 1)$ is defined by the cubic polynomial:
$$p(t) = a_3 t^3 + a_2 t^2 + a_1 t + a_0$$

The coefficients $a_k$ are derived by constraining the polynomial to match the sample values at $t=0$ ($p_1$) and $t=1$ ($p_2$), while matching the continuous derivative tangents $\tau$:
$$\tau_1 = \frac{p_2 - p_0}{2}, \quad \tau_2 = \frac{p_3 - p_1}{2}$$

Solving the linear system yields the Catmull-Rom basis matrix:
$$\begin{bmatrix} a_3 \\ a_2 \\ a_1 \\ a_0 \end{bmatrix} = \frac{1}{2} \begin{bmatrix} -1 & 3 & -3 & 1 \\ 2 & -5 & 4 & -1 \\ -1 & 0 & 1 & 0 \\ 0 & 2 & 0 & 0 \end{bmatrix} \begin{bmatrix} p_0 \\ p_1 \\ p_2 \\ p_3 \end{bmatrix}$$

Expanding into sample operations:
$$\begin{aligned}
a_0 &= p_1 \\
a_1 &= \frac{1}{2} (-p_0 + p_2) \\
a_2 &= \frac{1}{2} (2p_0 - 5p_1 + 4p_2 - p_3) \\
a_3 &= \frac{1}{2} (-p_0 + 3p_1 - 3p_2 + p_3)
\end{aligned}$$

Because the tangent derivatives match across adjacent intervals $[p_{k-1}, p_k]$ and $[p_k, p_{k+1}]$, the reconstructed continuous waveform has $C^1$ continuity (a continuous first derivative). This eliminates spectral discontinuities (clicks, pops, and high-frequency distortion) across chunk boundaries.

### 2.3 Fractional Phase Accumulator Mechanics
To resample from an input rate $F_{\text{in}} = 24,000\text{ Hz}$ to an output rate $F_{\text{out}} = 48,000\text{ Hz}$, the resampler steps through time by the exact rational ratio:
$$\text{ratio} = \frac{F_{\text{in}}}{F_{\text{out}}} = \frac{24,000}{48,000} = 0.5$$

For each generated output sample, the position pointer advances:
$$\text{pos}_{k+1} = \text{pos}_k + \text{ratio}$$
* For output sample 0: $\text{pos}_0 = 0.0 \implies \text{index} = 0, \text{frac} = 0.0 \implies p(0) = p_1$
* For output sample 1: $\text{pos}_1 = 0.5 \implies \text{index} = 0, \text{frac} = 0.5 \implies p(0.5)$
* For output sample 2: $\text{pos}_2 = 1.0 \implies \text{index} = 1, \text{frac} = 0.0 \implies p(0) = p_2$

Every input sample at 24kHz is interpolated into two output samples at 48kHz. The fractional remainder $\text{phase} = \text{pos} - \lfloor \text{pos} \rfloor$ is preserved across chunk boundaries in `self.phase`, ensuring phase coherence across packet arrivals.

---

## 3. Eliminating Audio Distortion: Mathematics of Soft-Knee Compression

### 3.1 The Flaw of Peak-Inversion Makeup Gain
The previous normalization implementation computed dynamic makeup gain per chunk:
$$\text{gain} = \min\left(4.0, \frac{0.89}{\text{peak}_{\text{utterance}}}\right)$$

Consider speech initiation:
1. **Chunk 1 (Phonetic Ingestion):** The speaker inhales or produces a soft unvoiced fricative (e.g., /s/, /f/, /th/). The maximum absolute sample is:
   $$\text{peak}_{\text{chunk}} = 0.08$$
2. **Gain Inflation:** The algorithm calculates:
   $$\text{gain} = \min\left(4.0, \frac{0.89}{0.08}\right) = \min(4.0, 11.125) = 4.0 \quad (+12.04 \text{ dB})$$
   All samples in Chunk 1 are multiplied by $4.0\times$.
3. **Vocalic Burst (Clipping Saturation):** In the latter half of the chunk, a voiced vowel begins, reaching raw amplitude $0.55$:
   $$s_{\text{boosted}} = 0.55 \times 4.0 = 2.20$$
4. **Hard Clamping:** When filled into the DAC buffer via `clamp(-1.0, 1.0)`:
   $$s_{\text{out}} = \min(1.0, \max(-1.0, 2.20)) = 1.0$$

The waveform peaks are sheared flat. The Fourier expansion of a clipped sine wave introduces odd harmonic distortion:
$$x_{\text{square}}(t) = \frac{4}{\pi} \sum_{k=1,3,5,\dots}^{\infty} \frac{1}{k} \sin(k \omega_0 t)$$
These high-amplitude odd harmonics ($3\omega_0, 5\omega_0, 7\omega_0, \dots$) produce harsh, grating digital distortion.

### 3.2 The Hyperbolic Tangent Soft-Knee Transfer Function
To prevent hard clipping while ensuring quiet speech is amplified cleanly, we designed an analog-modeled, non-linear transfer function $y(x)$:
$$y(x) = \begin{cases} 
x & \text{for } |x| \le x_{\text{knee}} \\
\text{sgn}(x) \left[ x_{\text{knee}} + W \cdot \tanh\left(\frac{|x| - x_{\text{knee}}}{W}\right) \right] & \text{for } |x| > x_{\text{knee}}
\end{cases}$$
where:
* $x_{\text{knee}} = 0.89$ (the linear limit, corresponding to $-1.01\text{ dBFS}$)
* $W = 0.09$ (the compression width window)
* Maximum output limit: $\lim_{x \to \infty} y(x) = x_{\text{knee}} + W \cdot 1.0 = 0.89 + 0.09 = 0.98$ ($-0.176\text{ dBFS}$)

#### Mathematical Proof of No Hard Clipping:
1. **$C^1$ Smooth Transition:**
   At $|x| = x_{\text{knee}} = 0.89$:
   $$y(0.89) = 0.89 + 0.09 \cdot \tanh(0) = 0.89$$
   The first derivative from above is:
   $$\left.\frac{dy}{dx}\right|_{x = 0.89^+} = 1.0 - \tanh^2(0) = 1.0$$
   The derivative matches the linear region slope ($\frac{dy}{dx} = 1.0$). There is zero derivative discontinuity.
2. **Strict Boundedness Below Full Scale:**
   Because $\forall z \in \mathbb{R}, |\tanh(z)| < 1.0$:
   $$|y(x)| < 0.89 + 0.09 = 0.98 < 1.0$$
   No sample can reach or exceed $1.0\text{ FS}$. Digital square-wave truncation is eliminated.

---

## 4. Why Audio Parts Were Being Cut: Buffer Dynamics

### 4.1 Utterance Boundary Truncation Bug
The previous implementation contained an aggressive drift-trimming routine inside `flush_resamplers()`:
```rust
let max_boundary_len = target_len + ((out_rate as usize * 500) / 1000);
if ring.len() > max_boundary_len {
    let excess = ring.len() - max_boundary_len;
    for _ in 0..excess { ring.pop_front(); }
}
```
For an output rate of $48,000\text{ Hz}$ and a target buffer of $100\text{ ms}$:
$$\text{max\_boundary\_len} = 4,800 + 24,000 = 28,800 \text{ samples } (600 \text{ ms})$$

#### The Failure Mechanism:
* A user speaks a standard sentence: *"Good morning, let us review the proposal"* ($\sim 3.0\text{ seconds}$ duration).
* Ollalink synthesizes the audio and transmits the chunks over WebSocket in a compressed burst of $250\text{ ms}$.
* The total samples pushed into the ring buffer:
  $$N = 3.0 \times 48,000 = 144,000 \text{ samples}$$
* The final frame carries `last: true`, immediately invoking `flush_resamplers()`.
* At $t = 250\text{ ms}$, the physical DAC has consumed only $250\text{ ms} \times 48,000 = 12,000\text{ samples}$.
* The unplayed audio remaining in the ring is:
  $$N_{\text{pending}} = 144,000 - 12,000 = 132,000 \text{ samples } (2.75 \text{ seconds})$$
* `flush_resamplers()` executed:
  $$\text{excess} = 132,000 - 28,800 = 103,200 \text{ samples } (2.15 \text{ seconds})$$
  It popped 103,200 samples from the front of the queue.
* **Result:** The first $2.15\text{ seconds}$ of the $3.0\text{ second}$ sentence were discarded. The listener heard only the final word *"proposal"*.

#### The Fix:
The safe boundary was expanded to $8.0\text{ seconds}$ ($384,000\text{ samples}$), and the buffer capacity to $12.0\text{ seconds}$. Sentences under $8\text{ seconds}$ are preserved in their entirety.

---

### 4.2 Hardware Callback Underrun Hysteresis
On Windows, CPAL configures WASAPI with typical callback buffer sizes of $M_{\text{cb}} = 128 \text{ samples}$. At $F_s = 48,000\text{ Hz}$, the callback periodicity $T_{\text{cb}}$ is:
$$T_{\text{cb}} = \frac{128}{48,000} \approx 2.667 \text{ milliseconds}$$

The previous code reverted playback state to pre-buffering after 15 consecutive empty callbacks:
$$t_{\text{timeout}} = 15 \times 2.667 \text{ ms} = 40.0 \text{ milliseconds}$$

Public Internet WebSocket connections exhibit typical jitter packet delays of $40\text{ to }120\text{ ms}$. If an audio packet was delayed by $50\text{ ms}$:
1. The ring buffer ran empty for 15 callbacks ($40\text{ ms}$).
2. `playing` flipped to `false`.
3. When the next packet arrived, the player clamped output to silence, waiting for the full pre-buffer threshold ($100\text{–}150\text{ ms}$) to refill.
4. **Result:** Speech was chopped into disjointed syllables with audible silence gaps.

#### The Fix:
The hysteresis threshold was increased from 15 to **80 consecutive callbacks**:
$$t_{\text{hysteresis}} = 80 \times 2.667 \text{ ms} \approx 213.3 \text{ milliseconds}$$
Normal network jitter gaps (up to $200\text{ ms}$) are bridged seamlessly without resetting the playback state.

---

### 4.3 Hardware Capture Ring Overruns
In `run_sender_watch`, outbound microphone transmission was paced using a blocking sleep:
```rust
if pending.len() >= chunk_samples {
    if now < scheduled {
        tokio::time::sleep(scheduled - now).await; // Slept for up to 500ms
    }
    // send frame
}
```
While Tokio was asleep for $500\text{ ms}$, the single-threaded loop could not pop samples from the hardware ring buffer `ringbuf::HeapCons<f32>`.
* In $500\text{ ms}$, the microphone hardware pushes $24,000\text{ samples}$.
* Any OS thread scheduling delay or IPC overhead caused the ring buffer to fill and overflow (`CAPTURE_OVERRUNS`), dropping thousands of raw microphone samples before transmission.

#### The Fix:
The pacing loop was decoupled into non-blocking micro-slices:
```rust
if now < scheduled {
    tokio::time::sleep((scheduled - now).min(std::time::Duration::from_millis(5))).await;
    continue;
}
```
The hardware capture ring buffer is emptied every $5\text{ ms}$ into `pending`. The hardware buffer never accumulates more than $240\text{ samples}$ ($5\text{ ms}$), completely eliminating capture overruns.

---

## 5. Architectural Summary of the Audio Pipeline

```mermaid
flowchart TD
    subgraph Capture [Microphone Uplink (CPAL Input)]
        A[Microphone HW: 48kHz] -->|WASAPI Callback| B[Lock-Free RingBuffer]
        B -->|Every 5ms Drain| C[Resampler: 48k -> 16k]
        C --> D[Uplink Soft Knee Limiter]
        D -->|500ms Paced Frames| E[Relay WebSocket Client]
    end

    subgraph Relay [Server Router & Normalization]
        E --> F[Relay Server]
        F -->|Upstream Config: 16k| G[Ollalink Sound-Stream API]
        G -->|Audio Stream| H[Event Parser: translateEvent]
        H -->|Canonical Lang Map| I{Target Lang Check}
        I -->|en, hi| J[Stream Lane: 48kHz]
        I -->|es, fr, de, zh, ar, pt, ru, kn| K[Multilingual Lane: 24kHz]
        J --> L[Downlink Audio Message]
        K --> L
    end

    subgraph Playback [Playback Downlink (CPAL Output)]
        L --> M[Rust WebSocket Reader]
        M -->|Header + PCM Queue| N[JitterPlayer: push_audio_with_rate]
        N -->|Catmull-Rom Spline| O[Resampler: 24k/48k -> DAC Rate]
        O --> P[Soft-Knee Tanh Compressor: Max 0.98]
        P --> Q[12-Second Jitter Ring]
        Q -->|80-Callback Hysteresis| R[fill_into Callback]
        R --> S[Speaker Hardware: 48kHz DAC]
    end
```

---

## 6. Long-Session Stability Analysis (20+ Minutes)

During continuous 20+ minute sessions, physical clocks naturally drift due to quartz oscillator tolerances ($\pm 20\text{ to }50 \text{ ppm}$):
$$\Delta t_{\text{drift}} = t_{\text{session}} \times \delta_{\text{crystal}}$$
For a 20-minute call ($1200\text{ seconds}$) at $50\text{ ppm}$:
$$\Delta t_{\text{drift}} = 1200 \times 0.000050 = 0.060 \text{ seconds} = 60 \text{ milliseconds}$$

Because the jitter buffer provides a $12\text{-second}$ operating envelope with an $8\text{-second}$ boundary threshold, a $60\text{ ms}$ drift across 20 minutes represents less than $0.75\%$ of buffer headroom. Natural conversational pauses between utterances drain the ring to zero, resetting drift accumulation to baseline automatically without dropping audio frames.
