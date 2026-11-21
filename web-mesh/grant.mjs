export const readGrantToken = /^emr_[a-f0-9]{64}$/;

export function isHttpLoopback(url) {
  return url.protocol === 'http:' && ['127.0.0.1', 'localhost', '[::1]'].includes(url.hostname);
}

export function isExactOrigin(url, raw, allowLocal) {
  return url.origin === raw && !url.username && !url.password &&
    (url.protocol === 'https:' || (allowLocal && isHttpLoopback(url)));
}
