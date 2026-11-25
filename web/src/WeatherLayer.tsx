import { WEATHER_FIELDS, type WeatherGrid, type WeatherMode } from './weather'
import { rampCss, rampTicks, regionsOf } from './weatherField'
import type { Effects } from './weatherGl'
import './weather.css'

/** each animated effect's toggle (the animation itself is weatherMap.ts's, as the owner chose it) */
const EFFECTS: { key: keyof Effects; label: string; title: string }[] = [
  { key: 'wind', label: 'Wind', title: 'Wind: drawn as moving streaks' },
  { key: 'fog', label: 'Fog', title: 'Fog: low visibility' },
  { key: 'rain', label: 'Rain', title: 'Rain: drawn only' },
  { key: 'cloud', label: 'Cloud', title: 'Cloud: drawn only' },
]

/** A weather colour field on the map and its scale (the Weather view's, one click away). */
export function ColourField({ mode, onMode, grid }: { mode: WeatherMode; onMode: (m: WeatherMode) => void; grid: WeatherGrid | null }) {
  const regions = regionsOf(grid)
  const label = mode !== 'off' ? WEATHER_FIELDS[mode] : null
  return <div className="weather-field"><label>Colour field on the map <select aria-label="Weather colour field" value={mode} onChange={e => onMode(e.target.value as WeatherMode)}>
    <option value="off">Off</option>{Object.entries(WEATHER_FIELDS).map(([key, v]) => <option key={key} value={key}>{v.label}</option>)}
  </select></label>
    {mode !== 'off' && label && regions && <div className="weather-ramp" role="img" aria-label={`${label.label} colour scale, ${label.ticks[0]} to ${label.ticks[label.ticks.length - 1]} ${label.unit}; faintly hatched where unavailable`}>
      <div className="scale"><i className="bar" style={{ background: rampCss(mode) }} />
        <div className="ticks">{rampTicks(mode).map(t => <span key={t.value} style={{ left: `${t.at * 100}%` }}>{t.label}</span>)}</div></div>
      <span className="unit">{label.unit}</span>
      <span className="none"><i className="hatch" />none</span>
    </div>}</div>
}

/** The map's weather bar, one quiet row: the animated effects' toggles, the light only when it
 *  a word only when the weather is stale or unavailable. The colour field and the figures are the
 *  Weather view's (the top bar's Layers). */
export function WeatherLegend({ mode, effects, onEffects, grid, error }: {
  mode: WeatherMode
  effects: Effects; onEffects: (fx: Effects) => void; grid: WeatherGrid | null
  error: string | null
}) {
  const anyOn = mode !== 'off' || Object.values(effects).some(Boolean)
  const regions = !error ? regionsOf(grid) : null
  // stale weather is a warning (what is drawn would mislead); a run with no weather source is just
  // said, quietly: nothing in it is wrong
  const stale = !!error || !!grid?.stale
  const status = error ?? (!anyOn ? null : grid?.stale ? 'Stale weather: hidden' : !regions ? (grid && !grid.available ? 'no weather data in this run' : 'no weather in this view') : null)
  return <aside className="weather-legend" aria-label="Weather map layer">
    <div><span className="weather-title">Weather</span>
      <span className="weather-toggles" role="group" aria-label="Weather effects">{EFFECTS.map(e =>
        <button key={e.key} className={`chip ${effects[e.key] ? 'on' : ''}`} aria-pressed={effects[e.key]} title={e.title}
          onClick={() => onEffects({ ...effects, [e.key]: !effects[e.key] })}>{e.label}</button>)}</span>
      {status && <span className={`weather-status${stale ? ' warn' : ''}`}>{status}</span>}</div>
  </aside>
}
