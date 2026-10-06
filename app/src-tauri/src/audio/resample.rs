//! audio/resample.rs - high-quality continuous streaming resampler powered by Rubato.
use anyhow::{bail, Result};
use rubato::{Resampler as RubatoResamplerTrait, FastFixedIn, PolynomialDegree};

/// Streaming audio resampler powered by Rubato with native bypass and zero-lag flushing.
pub struct RubatoResampler {
    in_rate: u32,
    out_rate: u32,
    resampler: Option<FastFixedIn<f32>>,
    chunk_size: usize,
    pending: Vec<f32>,
    total_in: u64,
    total_out: u64,
}

impl RubatoResampler {
    pub fn new(in_rate: u32, out_rate: u32) -> Result<Self> {
        if in_rate == 0 || out_rate == 0 {
            bail!("sample rates must be positive: in_rate={}, out_rate={}", in_rate, out_rate);
        }
        let (resampler, chunk_size) = if in_rate == out_rate {
            (None, 0)
        } else {
            let chunk = 16;
            let ratio = out_rate as f64 / in_rate as f64;
            let r = FastFixedIn::<f32>::new(
                ratio,
                ratio.max(1.0) * 1.5,
                PolynomialDegree::Cubic,
                chunk,
                1,
            )?;
            (Some(r), chunk)
        };

        Ok(Self {
            in_rate,
            out_rate,
            resampler,
            chunk_size,
            pending: Vec::with_capacity(512),
            total_in: 0,
            total_out: 0,
        })
    }

    /// Process a slice of mono f32 samples.
    /// If in_rate == out_rate, returns input immediately with zero overhead.
    pub fn process(&mut self, input: &[f32]) -> Result<Vec<f32>> {
        if input.is_empty() {
            return Ok(Vec::new());
        }
        let resampler = match self.resampler.as_mut() {
            Some(r) => r,
            None => return Ok(input.to_vec()), // Native rate bypass!
        };

        self.total_in += input.len() as u64;
        self.pending.extend_from_slice(input);
        let mut output = Vec::new();

        while self.pending.len() >= self.chunk_size {
            let chunk: Vec<f32> = self.pending.drain(..self.chunk_size).collect();
            let resampled = RubatoResamplerTrait::process(resampler, &[&chunk], None)?;
            output.extend_from_slice(&resampled[0]);
        }

        self.total_out += output.len() as u64;
        Ok(output)
    }

    /// Flush any remaining samples in pending buffer and drain filter delay on utterance boundary.
    pub fn flush(&mut self) -> Result<Vec<f32>> {
        let resampler = match self.resampler.as_mut() {
            Some(r) => r,
            None => return Ok(Vec::new()),
        };

        let mut output = Vec::new();
        let ratio = self.out_rate as f64 / self.in_rate as f64;
        let expected_total_out = ((self.total_in as f64) * ratio).round() as u64;

        // Process any remainder in pending
        while !self.pending.is_empty() {
            let mut chunk = self.pending.clone();
            chunk.resize(self.chunk_size, 0.0);
            self.pending.clear();
            let resampled = RubatoResamplerTrait::process(resampler, &[&chunk], None)?;
            output.extend_from_slice(&resampled[0]);
        }

        // If we are still short of expected_total_out due to filter delay, pump a silent chunk
        while (self.total_out + output.len() as u64) < expected_total_out {
            let silent = vec![0.0f32; self.chunk_size];
            let resampled = RubatoResamplerTrait::process(resampler, &[&silent], None)?;
            let slice = &resampled[0];
            let needed = (expected_total_out - (self.total_out + output.len() as u64)) as usize;
            let take = needed.min(slice.len());
            output.extend_from_slice(&slice[..take]);
            if take == 0 {
                break;
            }
        }

        // Trim any overshoot to match expected exactly
        if (self.total_out + output.len() as u64) > expected_total_out {
            let allowed = (expected_total_out.saturating_sub(self.total_out)) as usize;
            output.truncate(allowed);
        }

        self.total_out += output.len() as u64;
        // Reset counters for next utterance
        self.total_in = 0;
        self.total_out = 0;
        Ok(output)
    }

    #[allow(dead_code)]
    pub fn in_rate(&self) -> u32 { self.in_rate }
    #[allow(dead_code)]
    pub fn out_rate(&self) -> u32 { self.out_rate }
}

// Preserve existing Resampler alias for backward compatibility
pub type Resampler = RubatoResampler;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resample_same_rate_bypass() {
        let mut r = RubatoResampler::new(48000, 48000).unwrap();
        let input = vec![0.1f32, 0.2, 0.3, 0.4];
        let out = r.process(&input).unwrap();
        assert_eq!(out, input);
    }

    #[test]
    fn test_rubato_24k_to_48k_streaming_and_flush() {
        let mut r = RubatoResampler::new(24000, 48000).unwrap();
        let out1 = r.process(&vec![0.5f32; 240]).unwrap();
        let out2 = r.flush().unwrap();
        let total = out1.len() + out2.len();
        println!("out1: {}, out2: {}, total: {}", out1.len(), out2.len(), total);
        assert_eq!(total, 480);
    }

    #[test]
    fn test_rubato_arbitrary_sizes() {
        let mut r = RubatoResampler::new(48000, 44100).unwrap();
        let mut total = 0;
        total += r.process(&vec![0.5f32; 100]).unwrap().len();
        total += r.process(&vec![0.5f32; 800]).unwrap().len();
        total += r.process(&vec![0.5f32; 200]).unwrap().len();
        total += r.flush().unwrap().len();
        let expected = (1100.0f64 * 44100.0 / 48000.0).round() as usize;
        assert_eq!(total, expected);
    }
}

