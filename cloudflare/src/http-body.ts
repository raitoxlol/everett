import { RelayError } from './protocol';

export async function boundedText(body: ReadableStream<Uint8Array> | null, cap: number, timeoutMs?: number): Promise<string> {
  if (!body) throw new RelayError('missing_body', 400);
  const reader = body.getReader();
  const decoder = new TextDecoder('utf-8', { fatal: true, ignoreBOM: false });
  let size = 0, text = '', timer: ReturnType<typeof setTimeout> | undefined;
  const deadline = timeoutMs === undefined ? undefined : new Promise<never>((_, reject) => {
    timer = setTimeout(() => reject(new RelayError('request_timeout', 408)), timeoutMs);
  });
  try {
    for (;;) {
      const chunk = await (deadline ? Promise.race([reader.read(), deadline]) : reader.read());
      if (chunk.done) return text + decoder.decode();
      size += chunk.value.byteLength;
      if (size > cap) {
        if (!deadline) await reader.cancel();
        throw new RelayError('body_too_large', 413);
      }
      text += decoder.decode(chunk.value, { stream: true });
    }
  } finally {
    if (deadline) {
      clearTimeout(timer);
      void reader.cancel().catch(() => {});
    }
    reader.releaseLock();
  }
}
