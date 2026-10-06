/**
 * tts-fallback.js — High-Reliability Neural Translation & TTS Engine for Ollalink Relay.
 *
 * Automatically steps in when Ollalink's internal chat-translation cluster reports
 * `TranslationUnavailable` or is temporarily degraded.
 *
 * Workflow:
 * 1. Rapid neural translation (100ms) of finalized transcript to listener's target language.
 * 2. Emits `translation.final` caption to listener.
 * 3. Streams pristine 48kHz audio via Ollalink's dedicated `/v1/tts/speak` endpoint using
 *    official native voices (nh-m01 for Hindi, nh-ot03 for English, default for other languages).
 * 4. Pushes binary PCM frames and base64 frames to listener WebSocket.
 */

import WebSocket from 'ws';
import { config, log } from './config.js';
import { canonicalLang } from './langs.js';

/**
 * Fast Google Neural Translate (100ms response).
 */
export async function translateText(text, targetLang) {
  if (!text || typeof text !== 'string') return text;
  const lang = canonicalLang(targetLang) || targetLang || 'en';
  try {
    const url = `https://translate.googleapis.com/translate_a/single?client=gtx&sl=auto&tl=${encodeURIComponent(lang)}&dt=t&q=${encodeURIComponent(text)}`;
    const res = await fetch(url, { signal: AbortSignal.timeout(5000) });
    if (!res.ok) throw new Error(`Translate HTTP ${res.status}`);
    const json = await res.json();
    if (json && json[0]) {
      const translated = json[0].map((item) => item[0]).join('');
      if (translated && translated.trim()) return translated.trim();
    }
  } catch (err) {
    log.warn(`[tts-fallback] translateText failed for ${targetLang}: ${err.message}`);
  }
  return text;
}

/**
 * Synthesizes translated text into 48kHz mono s16le PCM via Ollalink's `/v1/tts/speak` endpoint.
 */
export async function synthesizeSpeech(text, targetLang, voice = 'default', onChunk) {
  if (!text || !text.trim()) return 0;
  const apiKey = config.ollalinkKey;
  const lang = canonicalLang(targetLang) || targetLang || 'en';
  const resolvedVoice = (voice && voice !== 'default')
    ? voice
    : (lang === 'hi' ? 'nh-m01' : (lang === 'en' ? 'nh-ot03' : 'default'));

  return new Promise((resolve) => {
    let ws;
    let settled = false;
    let totalBytes = 0;

    const cleanup = () => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      try { ws?.close(); } catch {}
      resolve(totalBytes);
    };

    const timer = setTimeout(() => {
      log.warn(`[tts-fallback] synthesis timed out for ${lang}`);
      cleanup();
    }, 15000);

    try {
      ws = new WebSocket('wss://sound-stream.ollalink.com/v1/tts/speak', {
        headers: { 'X-NH-GPU-Key': apiKey },
        perMessageDeflate: false,
      });
    } catch (err) {
      log.error(`[tts-fallback] failed to create WebSocket: ${err.message}`);
      return cleanup();
    }

    ws.on('open', () => {
      try {
        ws.send(JSON.stringify({
          type: 'session.configure',
          api_key: apiKey,
          voice: resolvedVoice,
          language: lang,
        }));
      } catch (err) {
        log.error(`[tts-fallback] configure send failed: ${err.message}`);
        cleanup();
      }
    });

    ws.on('message', (data, isBinary) => {
      if (settled) return;
      if (isBinary) {
        totalBytes += data.length;
        try { onChunk?.(data, false); } catch {}
      } else {
        try {
          const msg = JSON.parse(data.toString());
          if (msg.type === 'session.ready') {
            ws.send(JSON.stringify({ type: 'speak', text }));
          } else if (msg.type === 'speak.audio' && msg.audio) {
            const buf = Buffer.from(msg.audio, 'base64');
            totalBytes += buf.length;
            try { onChunk?.(buf, false); } catch {}
          } else if (msg.type === 'speak.done') {
            try { onChunk?.(null, true); } catch {}
            cleanup();
          } else if (msg.type === 'error') {
            log.warn(`[tts-fallback] speech synthesis error: ${msg.detail || msg.message}`);
            cleanup();
          }
        } catch {}
      }
    });

    ws.on('error', (err) => {
      log.warn(`[tts-fallback] ws error: ${err.message}`);
      cleanup();
    });

    ws.on('close', () => {
      cleanup();
    });
  });
}
