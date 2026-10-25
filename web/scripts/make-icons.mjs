#!/usr/bin/env node
// App icons from the vortex-math figure used in the jordivallverdu.com logo: `modulus` points on a
// circle (starting at the top), point i joined to (i * multiplier) % modulus. Here mod 9 ×2 (the
// 1-2-4-8-7-5 doubling loop) plus the 3-6-9 triangle, black lines on white.
//   node scripts/make-icons.mjs            → web/icons/*.svg
//   node scripts/make-icons.mjs --png      → also the PNG sizes (macOS: headless Chrome + sips)
import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const MODULUS = 9, MULTIPLIER = 2, EXTRA = [[0, 3], [3, 6], [6, 0]];
const OUT = path.join(path.dirname(fileURLToPath(import.meta.url)), '..', 'icons');

export function vortexSvg({ size = 512, radius = 0.34, stroke = 0.028, dot = 0.038, bg = '#ffffff', ink = '#111111' } = {}) {
  const c = size / 2, R = size * radius, f = n => n.toFixed(2);
  const pts = Array.from({ length: MODULUS }, (_, i) => {
    const a = -Math.PI / 2 + (2 * Math.PI * i) / MODULUS;
    return [c + R * Math.cos(a), c + R * Math.sin(a)];
  });
  const pairs = pts.map((_, i) => [i, (i * MULTIPLIER) % MODULUS]).filter(([a, b]) => a !== b).concat(EXTRA);
  const lines = pairs.map(([a, b]) => `<line x1="${f(pts[a][0])}" y1="${f(pts[a][1])}" x2="${f(pts[b][0])}" y2="${f(pts[b][1])}"/>`).join('');
  const dots = pts.map(([x, y]) => `<circle cx="${f(x)}" cy="${f(y)}" r="${f(size * dot)}"/>`).join('');
  return `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${size} ${size}" width="${size}" height="${size}">${bg ? `<rect width="${size}" height="${size}" fill="${bg}"/>` : ''}` +
    `<g stroke="${ink}" stroke-width="${f(size * stroke)}" stroke-linecap="round" stroke-linejoin="round">${lines}</g><g fill="${ink}">${dots}</g></svg>\n`;
}

fs.mkdirSync(OUT, { recursive: true });
// App icon (fits the maskable safe zone: drawing within 40% of the size from the centre).
fs.writeFileSync(path.join(OUT, 'icon.svg'), vortexSvg());
// Browser tab: bigger figure, heavier lines so it reads at 16–32 px.
fs.writeFileSync(path.join(OUT, 'favicon.svg'), vortexSvg({ size: 64, radius: 0.4, stroke: 0.06, dot: 0.07 }));
// Android status-bar badge: white on transparent.
fs.writeFileSync(path.join(OUT, 'badge.svg'), vortexSvg({ size: 96, radius: 0.4, stroke: 0.06, dot: 0.065, bg: null, ink: '#ffffff' }));

if (process.argv.includes('--png')) {
  const chrome = '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome';
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'pm-icons-'));
  const render = (svgFile, size, out, transparent = false) => {
    const html = path.join(tmp, 'icon.html');
    fs.writeFileSync(html, `<!DOCTYPE html><html><body style="margin:0;background:transparent"><img src="file://${path.join(OUT, svgFile)}" style="width:512px;height:512px;display:block"></body></html>`);
    const shot = path.join(tmp, 'shot.png');
    execFileSync(chrome, ['--headless=new', '--hide-scrollbars', '--window-size=512,512', '--timeout=3000', `--screenshot=${shot}`,
      ...(transparent ? ['--default-background-color=00000000'] : []), `file://${html}`], { stdio: 'ignore' });
    execFileSync('sips', ['-c', '512', '512', shot, '--out', shot], { stdio: 'ignore' });
    execFileSync('sips', ['-z', String(size), String(size), shot, '--out', path.join(OUT, out)], { stdio: 'ignore' });
  };
  render('icon.svg', 512, 'icon-512.png');
  render('icon.svg', 192, 'icon-192.png');
  render('icon.svg', 180, 'apple-touch-icon.png');
  render('badge.svg', 96, 'badge-96.png', true);
  fs.rmSync(tmp, { recursive: true, force: true });
}
console.log(`icons written to ${OUT}`);
