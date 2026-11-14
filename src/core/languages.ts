// Language catalogue: one table drives file detection, the editor's language
// picker, the VS Code language id handed to extensions, and which CodeMirror
// grammar to install.
//
// Ids are VS Code language ids on purpose. Extensions match documents by
// `languageId`, and snippets contributed for `python` only work if the document
// says `python` — not `py`.

export interface LanguageDef {
  id: string;
  label: string;
  /** Extensions without the dot, lower-case. */
  exts: string[];
  /** Exact file names (Dockerfile, Makefile, …). */
  names?: string[];
}

export const LANGUAGES: LanguageDef[] = [
  { id: 'plaintext', label: 'Plain Text', exts: ['txt', 'log', 'text'] },
  { id: 'python', label: 'Python', exts: ['py', 'pyw', 'pyi'] },
  { id: 'javascript', label: 'JavaScript', exts: ['js', 'mjs', 'cjs'] },
  { id: 'javascriptreact', label: 'JavaScript React', exts: ['jsx'] },
  { id: 'typescript', label: 'TypeScript', exts: ['ts', 'mts', 'cts'] },
  { id: 'typescriptreact', label: 'TypeScript React', exts: ['tsx'] },
  { id: 'json', label: 'JSON', exts: ['json'] },
  { id: 'jsonc', label: 'JSON with Comments', exts: ['jsonc'] },
  { id: 'markdown', label: 'Markdown', exts: ['md', 'markdown', 'mdx'] },
  { id: 'html', label: 'HTML', exts: ['html', 'htm', 'xhtml'] },
  { id: 'css', label: 'CSS', exts: ['css'] },
  { id: 'scss', label: 'SCSS', exts: ['scss'] },
  { id: 'less', label: 'Less', exts: ['less'] },
  { id: 'rust', label: 'Rust', exts: ['rs'] },
  { id: 'go', label: 'Go', exts: ['go'] },
  { id: 'java', label: 'Java', exts: ['java'] },
  { id: 'c', label: 'C', exts: ['c', 'h'] },
  { id: 'cpp', label: 'C++', exts: ['cpp', 'cc', 'cxx', 'hpp', 'hh', 'hxx'] },
  { id: 'csharp', label: 'C#', exts: ['cs'] },
  { id: 'php', label: 'PHP', exts: ['php'] },
  { id: 'ruby', label: 'Ruby', exts: ['rb', 'rake', 'gemspec'], names: ['Gemfile', 'Rakefile'] },
  { id: 'perl', label: 'Perl', exts: ['pl', 'pm'] },
  { id: 'lua', label: 'Lua', exts: ['lua'] },
  { id: 'shellscript', label: 'Shell Script', exts: ['sh', 'bash', 'zsh', 'fish'], names: ['.bashrc', '.zshrc'] },
  { id: 'powershell', label: 'PowerShell', exts: ['ps1', 'psm1'] },
  { id: 'bat', label: 'Batch', exts: ['bat', 'cmd'] },
  { id: 'yaml', label: 'YAML', exts: ['yml', 'yaml'] },
  { id: 'toml', label: 'TOML', exts: ['toml'] },
  { id: 'xml', label: 'XML', exts: ['xml', 'svg', 'plist'] },
  { id: 'sql', label: 'SQL', exts: ['sql'] },
  { id: 'ini', label: 'INI', exts: ['ini', 'cfg', 'conf', 'env'], names: ['.env', '.editorconfig'] },
  { id: 'dockerfile', label: 'Dockerfile', exts: ['dockerfile'], names: ['Dockerfile'] },
  { id: 'makefile', label: 'Makefile', exts: ['mk'], names: ['Makefile', 'makefile'] },
  { id: 'r', label: 'R', exts: ['r'] },
  { id: 'dart', label: 'Dart', exts: ['dart'] },
  { id: 'kotlin', label: 'Kotlin', exts: ['kt', 'kts'] },
  { id: 'swift', label: 'Swift', exts: ['swift'] },
  { id: 'vue', label: 'Vue', exts: ['vue'] },
  { id: 'svelte', label: 'Svelte', exts: ['svelte'] },
  { id: 'diff', label: 'Diff', exts: ['diff', 'patch'] },
];

const BY_EXT = new Map<string, string>();
const BY_NAME = new Map<string, string>();
for (const lang of LANGUAGES) {
  for (const e of lang.exts) if (!BY_EXT.has(e)) BY_EXT.set(e, lang.id);
  for (const n of lang.names ?? []) if (!BY_NAME.has(n)) BY_NAME.set(n, lang.id);
}

/** Language id for a path. Unknown extensions are plain text, never a guess. */
export function detectLanguage(pathOrName: string): string {
  const base = pathOrName.split(/[\\/]/).pop() ?? pathOrName;
  const byName = BY_NAME.get(base);
  if (byName) return byName;
  const dot = base.lastIndexOf('.');
  if (dot < 0) return 'plaintext';
  const ext = base.slice(dot + 1).toLowerCase();
  return BY_EXT.get(ext) ?? 'plaintext';
}

const BY_ID = new Map(LANGUAGES.map((l) => [l.id, l]));

export function languageLabel(id: string): string {
  return BY_ID.get(id)?.label ?? id;
}

export function isKnownLanguage(id: string): boolean {
  return BY_ID.has(id);
}

/** Languages that have a built-in runner, in menu order. */
export function runnerLanguages(): LanguageDef[] {
  const runnable = new Set([
    'python', 'javascript', 'javascriptreact', 'typescript', 'typescriptreact',
    'go', 'rust', 'java', 'c', 'cpp', 'php', 'ruby', 'perl', 'lua', 'r', 'dart',
    'shellscript', 'powershell', 'bat',
  ]);
  return LANGUAGES.filter((l) => runnable.has(l.id));
}
