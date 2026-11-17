const integer = new Intl.NumberFormat('en-GB')

export function formatNumber(n: number): string {
  return integer.format(n)
}

export function formatBytes(n: number): string {
  const units = ['B', 'KB', 'MB', 'GB', 'TB']
  let value = n
  let unit = 0
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024
    unit++
  }
  return unit === 0 ? `${n} B` : `${value.toFixed(1)} ${units[unit]}`
}

const words: Record<string, string> = {
  ip: 'IP',
  ips: 'IPs',
  url: 'URL',
  urls: 'URLs',
  asn: 'ASN',
  utm: 'UTM',
  ai: 'AI',
  id: 'ID',
  s2xx: '2xx',
  s3xx: '3xx',
  s4xx: '4xx',
  s5xx: '5xx',
}

/** `by_source_type` → `By source type`, `unique_ips` → `Unique IPs`. */
export function humanise(key: string): string {
  return key
    .split(/[_-]+/)
    .map((word, i) => words[word] ?? (i === 0 ? word.charAt(0).toUpperCase() + word.slice(1) : word))
    .join(' ')
}

/** `2026-11-14T00:00:05Z` → `2026-11-14 00:00 UTC`. */
export function formatUtc(iso: string | null): string {
  if (!iso) return '-'
  return `${iso.slice(0, 10)} ${iso.slice(11, 16)} UTC`
}

/** Unix seconds → `2026-11-14 00:00 UTC`. */
export function formatUnix(seconds: number): string {
  return formatUtc(new Date(seconds * 1000).toISOString())
}

export function fileName(path: string): string {
  return path.split(/[\\/]/).pop() ?? path
}
