declare global {
  namespace Cloudflare {
    interface GlobalProps {
      mainModule: typeof import('../src/index');
      durableNamespaces: 'OwnerRelay';
    }
  }
}
export {};
