// "Is this provider actually usable" — shared by the composer, the model
// picker and the boot sequence.
//
// Mirrors `AppConfig::default_provider_id()` in src-tauri/src/config.rs. The
// two sides have to agree: the renderer decides what the pickers show, the
// agent decides what it sends, and a mismatch is exactly how you get a chat
// that silently talks to `openai` (enabled by default, no key) while the user
// believes they configured `deepseek`.

import type { ProviderConfig } from '../types';

/** Local inference servers (ollama, llama.cpp, LM Studio) take no key. */
export function isLoopback(baseUrl: string): boolean {
  const u = (baseUrl || '').trim().toLowerCase();
  return (
    u.includes('://localhost') ||
    u.includes('://127.') ||
    u.includes('://0.0.0.0') ||
    u.includes('://[::1]') ||
    u.includes('://192.168.') ||
    u.startsWith('localhost') ||
    u.startsWith('127.')
  );
}

/** Switched on, has a base URL, and can authenticate. */
export function providerReady(p: ProviderConfig | null | undefined): boolean {
  if (!p) return false;
  if (!p.enabled) return false;
  if (!p.base_url.trim()) return false;
  return p.api_key.trim() !== '' || isLoopback(p.base_url);
}

/** Ready provider first, then any enabled one with a URL.
 *  The built-in Web2API provider is loopback-ready the moment the bridge is
 *  up, so it must not hijack a fresh install's default: its models are opt-in
 *  through the picker (and only appear for logged-in reverse proxies). */
export function pickDefaultProvider(providers: ProviderConfig[]): ProviderConfig | null {
  const list = providers.filter((p) => p.id !== 'web2api');
  return (
    list.find(providerReady) ??
    list.find((p) => p.enabled && p.base_url.trim() !== '') ??
    null
  );
}

/** Why a provider cannot be used yet, or null when it can. */
export function providerBlocker(p: ProviderConfig | null | undefined): 'off' | 'url' | 'key' | null {
  if (!p) return 'off';
  if (!p.enabled) return 'off';
  if (!p.base_url.trim()) return 'url';
  if (p.api_key.trim() === '' && !isLoopback(p.base_url)) return 'key';
  return null;
}
