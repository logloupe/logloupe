// Takes screenshots of the real desktop app by driving it through WebView2's
// remote debugging port. Use it only with a site built from synthetic data
// (see the gen_log example); never with client logs.
//
// 1. Start the app with debugging on and a scratch data directory:
//      set LOGLOUPE_DATA_DIR=<scratch dir containing a site>
//      set WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9333
//      target\release\logloupe-app.exe
// 2. node tools/screenshots.mjs 9333 <output-dir>
//
// The dates below match a log generated with --start 2026-11-08 --days 7.

import { writeFileSync } from 'node:fs'

const [port, outDir] = process.argv.slice(2)
const targets = await (await fetch(`http://127.0.0.1:${port}/json`)).json()
const page = targets.find((t) => t.type === 'page')
if (!page) throw new Error('no page target')
const ws = new WebSocket(page.webSocketDebuggerUrl)
await new Promise((resolve, reject) => {
  ws.onopen = resolve
  ws.onerror = reject
})

let nextId = 0
const pending = new Map()
ws.onmessage = (event) => {
  const message = JSON.parse(event.data)
  const waiter = pending.get(message.id)
  if (!waiter) return
  pending.delete(message.id)
  message.error ? waiter.reject(new Error(message.error.message)) : waiter.resolve(message.result)
}
const send = (method, params = {}) =>
  new Promise((resolve, reject) => {
    const id = ++nextId
    pending.set(id, { resolve, reject })
    ws.send(JSON.stringify({ id, method, params }))
  })
const sleep = (ms) => new Promise((r) => setTimeout(r, ms))

async function evaluate(expression) {
  const result = await send('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true })
  if (result.exceptionDetails) throw new Error(result.exceptionDetails.exception?.description ?? 'evaluate failed')
  return result.result.value
}

// A fixed viewport at 1.5x, so every shot is the same size and sharp.
const WIDTH = 1320
const HEIGHT = 890
await send('Emulation.setDeviceMetricsOverride', { width: WIDTH, height: HEIGHT, deviceScaleFactor: 1.5, mobile: false })

async function shot(name, clip) {
  const params = { format: 'png' }
  if (clip) params.clip = { ...clip, scale: clip.scale ?? 1 }
  const { data } = await send('Page.captureScreenshot', params)
  writeFileSync(`${outDir}/${name}.png`, Buffer.from(data, 'base64'))
  console.log('saved', name)
}

const helpers = `
  window.__t = {
    wait: (ms) => new Promise((r) => setTimeout(r, ms)),
    nav: async (label) => {
      const b = [...document.querySelectorAll('nav button')].find((x) => x.textContent.trim() === label)
      if (!b) throw new Error('no nav item ' + label)
      b.click()
      await window.__t.wait(300)
      await window.__t.settled()
    },
    // Wait until no report is loading and tables have rendered.
    settled: async () => {
      for (let i = 0; i < 80; i++) {
        await window.__t.wait(250)
        const busy = document.body.innerText.includes('Running report') || document.querySelector('.opacity-60')
        if (!busy && document.querySelector('table')) return
      }
    },
    setDate: (index, value) => {
      const input = document.querySelectorAll('header input[type=date]')[index]
      const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set
      setter.call(input, value)
      input.dispatchEvent(new Event('input', { bubbles: true }))
      input.dispatchEvent(new Event('change', { bubbles: true }))
    },
    rect: (el) => {
      const b = el.getBoundingClientRect()
      return { x: b.x, y: b.y, width: b.width, height: b.height }
    },
    section: (title) => [...document.querySelectorAll('main section')].find((s) => s.querySelector('h3')?.textContent.trim() === title),
  }
  true
`
await evaluate(helpers)

// Wait for the app to finish starting, then show the date range in the fields.
await evaluate(`window.__t.settled()`)
await evaluate(`window.__t.setDate(0, '2026-11-08'); window.__t.setDate(1, '2026-11-14'); window.__t.wait(600)`)
await evaluate(`window.__t.settled()`)

// Put the pointer over a chart point so the legend shows real values.
async function hoverChart(fraction = 0.72) {
  const box = await evaluate(`(() => { const o = document.querySelector('.u-over'); return o ? window.__t.rect(o) : null })()`)
  if (!box) return
  await send('Input.dispatchMouseEvent', {
    type: 'mouseMoved',
    x: Math.round(box.x + box.width * fraction),
    y: Math.round(box.y + box.height * 0.45),
  })
  await sleep(300)
}
const parkPointer = () => send('Input.dispatchMouseEvent', { type: 'mouseMoved', x: 700, y: 30 })

// 1. Overview
await evaluate(`window.__t.nav('Overview')`)
await hoverChart()
await shot('overview')

// 2. All bots, plus a cropped detail of the table for the hero.
await evaluate(`window.__t.nav('All bots')`)
await hoverChart()
await shot('bots')
await parkPointer()
const botTable = await evaluate(`(() => {
  const s = window.__t.section('By bot')
  s.scrollIntoView({ block: 'start' })
  const head = window.__t.rect(s)
  const rows = s.querySelectorAll('tbody tr')
  const sixth = window.__t.rect(rows[Math.min(5, rows.length - 1)])
  return { x: head.x, y: head.y, width: head.width, height: sixth.y + sixth.height - head.y }
})()`)
await sleep(300)
await shot('bots-detail', { ...botTable, scale: 2 })

// 3. AI crawlers
await evaluate(`document.querySelector('main').scrollTo(0, 0); window.__t.nav('AI crawlers')`)
await shot('ai-crawlers')

// 4. Fake bots, then the same page with rows selected and the nginx list showing.
await evaluate(`window.__t.nav('Fake bots')`)
await shot('fake-bots')
await evaluate(`(async () => {
  const s = window.__t.section('By IP')
  const boxes = [...s.querySelectorAll('tbody input[type=checkbox]')].slice(0, 5)
  for (const b of boxes) { b.click(); await window.__t.wait(60) }
  await window.__t.wait(200)
  const button = [...s.querySelectorAll('button')].find((b) => b.textContent.trim() === 'nginx deny')
  button.click()
  await window.__t.wait(900)
  s.scrollIntoView({ block: 'start' })
  await window.__t.wait(300)
})()`)
await shot('block-list')
const blockDetail = await evaluate(`(() => {
  const s = window.__t.section('By IP')
  const head = window.__t.rect(s)
  const rows = s.querySelectorAll('tbody tr')
  const last = window.__t.rect(rows[Math.min(5, rows.length - 1)])
  return { x: head.x, y: head.y, width: head.width, height: last.y + last.height - head.y }
})()`)
await shot('block-list-detail', { ...blockDetail, scale: 2 })

// 5. Query strings
await evaluate(`document.querySelector('main').scrollTo(0, 0); window.__t.nav('Query strings')`)
await shot('query-strings')

// 6. Bursts: the whole page, then just the incidents and their top paths.
await evaluate(`window.__t.nav('Bursts')`)
await shot('bursts')
const incidents = await evaluate(`(() => {
  const s = window.__t.section('Incidents')
  const head = window.__t.rect(s)
  const rows = s.querySelectorAll('tbody tr')
  const last = window.__t.rect(rows[Math.min(4, rows.length - 1)])
  return { x: head.x, y: head.y, width: head.width, height: last.y + last.height - head.y, rows: rows.length }
})()`)
console.log('incident rows on page:', incidents.rows)
await shot('bursts-detail', { x: incidents.x, y: incidents.y, width: incidents.width, height: incidents.height, scale: 2 })

// The day of the burst, by the hour, so the spike is visible in the chart.
await evaluate(`document.querySelector('main').scrollTo(0, 0); window.__t.setDate(0, '2026-11-12'); window.__t.setDate(1, '2026-11-12'); window.__t.wait(600)`)
await evaluate(`window.__t.nav('Overview')`)
await evaluate(`window.__t.settled()`)
await hoverChart(0.56)
await shot('overview-burst-day')
ws.close()
