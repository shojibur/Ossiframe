// vite.config.ts needs only this one Node function; avoids adding @types/node.
declare module "node:url" {
  export function fileURLToPath(url: URL | string): string;
}
