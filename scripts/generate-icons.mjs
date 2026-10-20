import { readFileSync, writeFileSync, mkdirSync, copyFileSync } from 'node:fs'
import { spawnSync } from 'node:child_process'
import { fileURLToPath } from 'node:url'
import { resolve } from 'node:path'

const root = fileURLToPath(new URL('../', import.meta.url))
const work = resolve(root, 'work/icons')
const source = resolve(root, 'src/assets/icon.svg')
const cli = resolve(root, 'node_modules/@tauri-apps/cli/tauri.js')
mkdirSync(work, { recursive: true })

function generate(input, output) {
  const result = spawnSync(process.execPath, [cli, 'icon', input, '--output', output], {
    cwd: root, stdio: 'inherit',
  })
  if (result.error) throw result.error
  if (result.status !== 0) throw new Error('Icon generation failed')
}

generate(source, resolve(work, 'standard'))
for (const name of ['32x32.png', '128x128.png', '128x128@2x.png', 'icon.ico']) {
  copyFileSync(resolve(work, 'standard', name), resolve(root, 'src-tauri/icons', name))
}

// Keep the canonical mark full-size in the UI; the Mac app icon needs an inset
// within its transparent canvas to match the optical size of neighboring Dock icons.
const svg = readFileSync(source, 'utf8')
const body = svg.replace(/^[\s\S]*?<svg\b[^>]*>/, '').replace(/<\/svg>\s*$/, '')
const macSource = resolve(work, 'macos.svg')
writeFileSync(macSource, `<svg xmlns="http://www.w3.org/2000/svg" width="1024" height="1024" viewBox="0 0 64 64"><g transform="translate(6.4 6.4) scale(.8)">${body}</g></svg>\n`)
generate(macSource, resolve(work, 'macos'))
copyFileSync(resolve(work, 'macos/icon.icns'), resolve(root, 'src-tauri/icons/icon.icns'))
