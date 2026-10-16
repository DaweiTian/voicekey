/* Render og.svg -> website/public/og.png (1200x630 social share image). */
const fs = require('fs')
const path = require('path')
const { Resvg } = require('@resvg/resvg-js')

const svg = fs.readFileSync(path.join(__dirname, 'og.svg'))
const resvg = new Resvg(svg, {
  fitTo: { mode: 'width', value: 1200 },
  background: 'rgba(0,0,0,0)',
  font: {
    fontDirs: ['C:/Windows/Fonts'],
    loadSystemFonts: true,
    defaultFontFamily: 'Microsoft YaHei',
  },
})
const png = resvg.render().asPng()
const target = path.join(process.argv[2], 'website', 'public', 'og.png')
fs.writeFileSync(target, png)
console.log('wrote', target, png.length, 'bytes')
