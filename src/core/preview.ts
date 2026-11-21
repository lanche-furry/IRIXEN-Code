// A stand-in config for running the renderer in a plain browser (`npm run dev`).
//
// Inside Tauri every value comes from Rust; without it the shell would mount
// empty, which makes UI work impossible. This is only used when the Tauri IPC
// is missing and we are in a dev build.

import type { AppConfig } from '../types';

export function previewConfig(): AppConfig {
  return {
    version: 1,
    onboarding_completed: false,
    close_to_tray: true,
    workspace: null,
    theme: 'light',
    language: 'zh-CN',
    proxy: '',
    providers: [
      {
        id: 'web2api',
        name: 'Web2API',
        protocol: 'openai',
        base_url: 'http://127.0.0.1:8791/v1',
        api_key: '',
        enabled: false,
        models_path: '/models',
        extra_headers: [],
        extra_body: {},
        note: '',
      },
      {
        id: 'nvidia',
        name: 'NVIDIA NIM',
        protocol: 'openai',
        base_url: 'https://integrate.api.nvidia.com/v1',
        api_key: '',
        enabled: false,
        models_path: '/models',
        extra_headers: [],
        extra_body: {},
        note: '',
      },
      {
        id: 'deepseek',
        name: 'DeepSeek',
        protocol: 'openai',
        base_url: 'https://api.deepseek.com/v1',
        api_key: 'sk-preview-0000000000000000',
        enabled: true,
        models_path: '/models',
        extra_headers: [],
        extra_body: {},
        note: '',
      },
      {
        id: 'anthropic',
        name: 'Anthropic',
        protocol: 'anthropic',
        base_url: 'https://api.anthropic.com/v1',
        api_key: 'sk-ant-preview',
        enabled: true,
        models_path: '/models',
        extra_headers: [],
        extra_body: {},
        note: '',
      },
      {
        id: 'gemini',
        name: 'Google Gemini',
        protocol: 'gemini',
        base_url: 'https://generativelanguage.googleapis.com/v1beta',
        api_key: '',
        enabled: false,
        models_path: '/models',
        extra_headers: [],
        extra_body: {},
        note: '',
      },
    ],
    active: { provider_id: 'deepseek', model: 'deepseek-flash', effort: 'auto' },
    effort: 'auto',
    agent: {
      max_iterations: 24,
      temperature: 0.7,
      system_prompt: '',
      auto_skills: true,
      compression: {
        max_context_tokens: 128000,
        threshold: 0.75,
        tool_result_age: 6,
        tool_result_cap: 4000,
        keep_recent: 10,
        digest_max_tokens: 800,
        enabled: true,
      },
    },
    mcp_servers: [],
    skills: { enabled: [], auto_match: true },
    custom_themes: [],
    bridge: {
      // The preview shell never enables the bridge: it is the one switch that
      // has to be a real decision, and a demo fixture is not one.
      enabled: false,
      full_access: false,
      auto_start: false,
      port: 8791,
      api_key: '',
      upstream: 'web',
      provider_id: '',
      model: 'claude-sonnet-4-5',
      // Real presets come from the Rust config on load; preview only needs a
      // shape-valid stub so the demo shell can render.
      sources: [],
      routes: [],
      default_source: 'claude',
      auto_ensure: true,
      routing_version: 2,
      claude_session_key: '',
      claude_org_id: '',
      claude_model: 'claude-sonnet-4-5',
    },
    extensions: { node_host: true, node_path: '', autostart_delay_ms: 1500 },
  };
}
