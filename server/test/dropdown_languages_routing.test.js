// server/test/dropdown_languages_routing.test.js
import { test } from 'node:test';
import assert from 'node:assert/strict';

process.env.PORT = '35299';
process.env.PUBLIC_BASE = 'ws://localhost:35299';
process.env.OLLALINK_DASHBOARD_KEY = 'sk_test_dummy';
process.env.OLLALINK_WS_URL = 'ws://127.0.0.1:35201/v1/speech/stream';
process.env.SESSION_SECRET = 'a'.repeat(64);
process.env.LOG_LEVEL = 'error';

const { normalizeLang } = await import('../src/langs.js');
const { translateEvent } = await import('../src/ollalink.js');

test('langs.js: dropdown language variations normalize to canonical targets', () => {
  const cases = [
    { in: 'eng', exp: 'en' },
    { in: 'english', exp: 'en' },
    { in: 'hin', exp: 'hi' },
    { in: 'hindi', exp: 'hi' },
    { in: 'spanish', exp: 'es' },
    { in: 'spa', exp: 'es' },
    { in: 'french', exp: 'fr' },
    { in: 'fra', exp: 'fr' },
    { in: 'chinese', exp: 'zh' },
    { in: 'german', exp: 'de' },
    { in: 'arabic', exp: 'ar' },
    { in: 'portugese', exp: 'pt' },
    { in: 'portuguese', exp: 'pt' },
    { in: 'russian', exp: 'ru' },
  ];

  for (const c of cases) {
    const norm = normalizeLang(c.in, 'target');
    assert.equal(norm, c.exp);
  }
});

test('ollalink.js: streaming PCM lane defaults to 48 kHz and batch WAV defaults to 24 kHz', () => {
  const multilingualLangs = ['es', 'fr', 'zh', 'de', 'ar', 'pt', 'ru', 'kn', 'hi', 'en'];
  for (const lang of multilingualLangs) {
    // 1. Streaming PCM lane defaults to 48000 Hz
    const evtPcm = translateEvent(JSON.stringify({
      type: 'translation.audio',
      audio_b64: Buffer.from([0, 0, 10, 0]).toString('base64'),
      target: lang,
      chunk_seq: 1,
    }));
    assert.equal(evtPcm.kind, 'audio');
    assert.equal(evtPcm.payload.sampleRate, 48000);
    assert.equal(evtPcm.payload.language, lang);

    // 2. Batch WAV lane defaults to 24000 Hz
    const evtWav = translateEvent(JSON.stringify({
      type: 'translation.audio',
      codec: 'wav',
      audio_b64: Buffer.from([0, 0, 10, 0]).toString('base64'),
      target: lang,
      chunk_seq: 1,
    }));
    assert.equal(evtWav.kind, 'audio');
    assert.equal(evtWav.payload.sampleRate, 24000);
  }
});
