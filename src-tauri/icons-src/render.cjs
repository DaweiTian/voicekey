/* 把 icon.svg 渲染为各处品牌位图（应用 favicon、README logo、官网 logo/站点图标）。
 * 依赖：npm i @resvg/resvg-js png-to-ico（临时目录安装即可，不必进仓库依赖）
 * 用法：node src-tauri/icons-src/render.cjs
 * 之后再运行：npm run tauri -- icon src-tauri/icons-src/icon-1024.png
 */
const fs = require('fs')
const path = require('path')
const { Resvg } = require('@resvg/resvg-js')
const pngToIcoModule = require('png-to-ico')
const pngToIco = pngToIcoModule.default || pngToIcoModule

const repo = path.resolve(__dirname, '..', '..')
const svg = fs.readFileSync(path.join(__dirname, 'icon.svg'))

const render = (width) =>
  new Resvg(svg, { fitTo: { mode: 'width', value: width }, background: 'rgba(0,0,0,0)' })
    .render()
    .asPng()

const sizes = [1024, 512, 192, 180, 48, 32, 16]
const buffers = {}
for (const size of sizes) {
  buffers[size] = render(size)
}

const copy = (from, to) => {
  const target = path.join(repo, to)
  fs.writeFileSync(target, buffers[from])
  console.log('wrote', target)
}

copy(1024, path.join('src-tauri', 'icons-src', 'icon-1024.png'))
copy(1024, path.join('public', 'zhiruo-icon-1024.png'))
copy(512, path.join('imgs', 'logo.png'))
copy(512, path.join('website', 'src', 'assets', 'logo.png'))
copy(32, path.join('website', 'public', 'favicon-32x32.png'))
copy(16, path.join('website', 'public', 'favicon-16x16.png'))
copy(180, path.join('website', 'public', 'apple-touch-icon.png'))
copy(192, path.join('website', 'public', 'android-chrome-192x192.png'))
copy(512, path.join('website', 'public', 'android-chrome-512x512.png'))

pngToIco([buffers[48], buffers[32], buffers[16]]).then((buf) => {
  const target = path.join(repo, 'website', 'public', 'favicon.ico')
  fs.writeFileSync(target, buf)
  console.log('wrote', target)
})
