// Rasterize assets/icon.png (the high-resolution transparent master) into
// every icon format Tauri needs.
// Usage: npm run icons
//
// The master is smooth vector-style artwork on a transparent plate, NOT pixel
// art. Every size is produced with a lanczos3 resample from one squared 1024px
// plate, so small taskbar sizes stay clean and large tiles stay smooth.
// The same 512px plate is copied into public/ for the renderer (boot splash,
// first-run wizard, editor empty state, titlebar).
import sharp from 'sharp';
import pngToIco from 'png-to-ico';
import { mkdir, writeFile } from 'node:fs/promises';
import path from 'node:path';

const root = process.cwd();
const srcPath = path.join(root, 'assets', 'icon.png');
const out = path.join(root, 'src-tauri', 'icons');
await mkdir(out, { recursive: true });

const meta = await sharp(srcPath).metadata();
if (!meta.width || !meta.height) throw new Error('unable to read icon master');
// Centre-crop to a square so a slightly rectangular master never gets squashed.
const side = Math.min(meta.width, meta.height);
const master = await sharp(srcPath)
  .ensureAlpha()
  .extract({
    left: Math.floor((meta.width - side) / 2),
    top: Math.floor((meta.height - side) / 2),
    width: side,
    height: side,
  })
  .resize(1024, 1024, { kernel: 'lanczos3', fit: 'fill' })
  .png()
  .toBuffer();

const render = (size) =>
  sharp(master).resize(size, size, { kernel: 'lanczos3', fit: 'fill' }).png().toBuffer();

const sizes = [16, 32, 48, 64, 128, 256, 512, 1024];
const buf = {};
for (const s of sizes) {
  buf[s] = s === 1024 ? master : await render(s);
  await writeFile(path.join(out, `icon-${s}.png`), buf[s]);
}

// canonical Tauri names
await writeFile(path.join(out, '32x32.png'), buf[32]);
await writeFile(path.join(out, '128x128.png'), buf[128]);
await writeFile(path.join(out, '128x128@2x.png'), buf[256]);
await writeFile(path.join(out, 'icon.png'), buf[512]);

// Windows Store logos (kept for future MSIX packaging)
const store = [
  ['Square30x30Logo.png', 30], ['Square44x44Logo.png', 44], ['Square71x71Logo.png', 71],
  ['Square89x89Logo.png', 89], ['Square107x107Logo.png', 107], ['Square142x142Logo.png', 142],
  ['Square150x150Logo.png', 150], ['Square284x284Logo.png', 284], ['Square310x310Logo.png', 310],
  ['StoreLogo.png', 50],
];
for (const [name, size] of store) {
  await writeFile(path.join(out, name), await render(size));
}

// The renderer shows the very same mark. Vite serves `public/` at the site
// root; 512px is plenty for a 96px empty-state tile and keeps the bundle and
// the first paint small.
await writeFile(path.join(root, 'public', 'irixen-icon.png'), buf[512]);

// ICO (Windows exe + taskbar)
const ico = await pngToIco([buf[16], buf[32], buf[48], buf[64], buf[128], buf[256]]);
await writeFile(path.join(out, 'icon.ico'), ico);

// ICNS (macOS) — PNG-based entries, valid per the icns container format
function icns(entries) {
  const total = 8 + entries.reduce((n, [, b]) => n + 8 + b.length, 0);
  const head = Buffer.alloc(8);
  head.write('icns', 0, 'ascii');
  head.writeUInt32BE(total, 4);
  const parts = [head];
  for (const [type, b] of entries) {
    const h = Buffer.alloc(8);
    h.write(type, 0, 'ascii');
    h.writeUInt32BE(8 + b.length, 4);
    parts.push(h, b);
  }
  return Buffer.concat(parts);
}
await writeFile(
  path.join(out, 'icon.icns'),
  icns([['ic07', buf[128]], ['ic08', buf[256]], ['ic09', buf[512]], ['ic10', buf[1024]]]),
);

console.log(`[irixen] icons written from ${meta.width}×${meta.height} master →`, path.relative(root, out));
