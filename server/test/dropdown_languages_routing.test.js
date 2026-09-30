// server/test/dropdown_languages_routing.test.js
import { test } from 'node:test';
import assert from 'node:assert/strict';

process.env.PORT = '35299';
process.env.PUBLIC_BASE = 'ws://localhost:35299';
process.env.OLLALINK_DASHBOARD_KEY = 'sk_test_dummy';
process.env.OLLALINK_WS_URL = 'ws://127.0.0.1:35201/v1/speech/stream';
process.env.SESSION_SECRET = 'a'.repeat(64);
process.env.LOG_LEVEL = 'error';

const { normalizeLang, canonicalLang } = await import('../src/langs.js');
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
    assert.equal(norm, c.exp, `Language '${c.in}' should normalize to '${c.exp}', got '${norm}'`);
  }
});

test('ollalink.js: multilingual targets guarantee 24 kHz to eliminate chipmunks', () => {
  const multilingualLangs = ['es', 'fr', 'zh', 'de', 'ar', 'pt', 'ru', 'kn'];
  for (const lang of multilingualLangs) {
    const evt = translateEvent(JSON.stringify({
      type: 'translation.audio',
      audio_b64: Buffer.from([0, 0, 10, 0]).toString('base64'),
      language: lang,
      chunk_seq: 1,
    }));

    assert.equal(evt.kind, 'audio');
    assert.equal(evt.payload.sampleRate, 24000, `Target '${lang}' must default to 24000 Hz, got ${evt.payload.sampleRate}`);
  }
});
