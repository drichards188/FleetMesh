import { type ReactNode, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react'
import { useBridgeClock, useHiddenAlerts, type Intent } from './decisions'
import { FleetPanel } from './FleetPanel'
import { InboxButton, InboxPanel, type InboxSelection, type InboxTab } from './InboxPanel'
import { EMPTY_PREVIEW, intentPoints, previewFeatures } from './inboxPreview'
import { MapView, type Draft, type MapLayers } from './MapView'
import { PlatformsPanel } from './PlatformsPanel'
import { TaskPanel } from './TaskPanel'
import type { Reveal } from './reveal'
import { VehicleDetailsWhenFull } from './VehicleDetails'
import type { LngLat, Vehicle } from './types'
import { isStale, vehicleNamer } from './types'
import { useBridge } from './useBridge'
import { ErrorBoundary } from './ErrorBoundary'
import { FloatingPanel, FloatLayer } from './FloatingPanel'
import { usePlatformLimits } from './taskDefaults'
import { estimated } from './estimate'
import { buildEstimateLayers, estimateLabelCandidates, estimateTooltip } from './estimateLayers'
import { PERF_ON, perfCommitted, perfPassiveBegin, perfRenderBegin, perfUpdateEnd } from './perf'
import { PerfOverlay } from './PerfOverlay'
import { QueueButton, QueuePanel } from './QueuePanel'
import { buildOrderLayers, orderLabelCandidates } from './orderLayers'
import { refreshAuth, useAuth } from './auth'
import { EnrollScreen, LoginScreen, OperatorBadge } from './Login'
import { WeatherLegend } from './WeatherLayer'
import { useWeatherGrid } from './useWeatherGrid'
import type { WeatherMode } from './weather'
import { regionsOf } from './weatherField'
import type { Effects, WeatherView } from './weatherMap'
import { saveEffects, savedEffects } from './weatherPrefs'
import { urgentInbox } from './inboxBadge'
import { CoverageControl } from './CoverageControl'
import { gridImage, type CoverageGrid, type LinkRouteDrawing } from './coverage'
import { buildStationLayers, EXCLUDED_PICK_LAYER } from './linkLayers'
import type { ExcludedMark } from './groupOrders'
import { nodeLossView } from './nodeLoss'
import { buildNodeRegionLayers } from './nodeLossLayers'
import { useNodes } from './useNodes'
import type { MapScreen } from './aloft'
import { useGroups } from './useGroups'
import { TaskingButton, TaskingPanel } from './TaskingPanel'
import { OfficerCompose, type ComposeInsert } from './OfficerCompose'
import { Sitrep } from './Sitrep'
import { askSentence } from './approvalText'
import { askFleet } from './approvalFleet'
import { useTasking } from './useTasking'
import { closeDock, FIT_PAD, fitPad, headsWithWaiting, toggleDock, type DockId } from './dock'
import { awaitingRows, officerShown, selectionInsert, turnOf } from './officer'
import { Awaiting, TurnCause } from './Awaiting'
import type { ExceptionContext } from './vehicleException'
import { clockText, type ScenarioClockAnswer, setWorldClock } from './scenarioClock'
import { GROUP_SCALE, groupScale, nextTrouble, troubleOf } from './topbar'
import { type ViewEntry, ViewsMenu } from './ViewsMenu'
// last, so each layout's own CSS comes after the parts' (layouts/parts.ts says what it may move)
import type { BarParts, ConsoleParts, MapSlot } from './layouts/parts'
import { LAYOUT, ShownLayout } from './layouts'
import { OffButton } from './OffButton'
import { OperatorLoad, ParcelsButton } from './OperatorLoad'
import { ParcelsPanel } from './ParcelsPanel'
import { useDelivery } from './useDelivery'
import { isLate, parcelPlaces, type ParcelPlace } from './delivery'
import { deliveryLabelCandidates } from './deliveryLabels'
import { buildDeliveryLayers, deliveryTooltip, PARCEL_PICK_LAYERS, type DeliveryData, type DeliveryDraw } from './deliveryLayers'
import { deliveryRoutes } from './deliveryRoutes'
import { suspectAreas } from './suspectAreas'
import { alertTracks, TRAFFIC_ALERT_KINDS } from './decisions'
import { perfPush } from './perf'
import { declutter, disagreementPairs, matchTruth, type FleetContext, type ShownTrack } from './traffic'
import { TrafficBar, type TrafficToggles } from './TrafficBar'
import { TrafficDetails } from './TrafficDetails'
import { buildTrafficLayers, trafficLabelCandidates, trafficTooltip, TRAFFIC_PICK_LAYERS, type TrafficLayerInput } from './trafficLayers'
import { trackTitle, trackTooltip } from './trafficText'
import { domainOf } from './types'
import { useTraffic, useTrafficSensors, useTrafficTruth } from './useTraffic'
// the fleet by home base (docs/design/fleet-organisation.md): the Fleet list's and the map's
import { useFleet } from './useFleet'
import { HubDetails } from './HubDetails'
import { hubDetails } from './hubSummary'
import { fallbackFleet, type FleetDoc } from './fleetModel'
import { baseLabelCandidates, type BaseMarker, collapseAtBases, EXPAND_ZOOM, SITE_ZOOM, toggleExpanded, WITH_OFFICER } from './fleetMap'
import { BASE_PICK_LAYERS, buildBaseLayers } from './fleetMapLayers'
import { latestOrders } from './orderWords'

/** The bridge's scenario clock (scenarioClock.ts, `scenario` in GET /api/weather): asked once the
 *  bridge answers, again on each reconnect (a restarted bridge has a new start) and each minute. A
 *  bridge without it is read as its own clock. */
function useScenarioClock(live: boolean): ScenarioClockAnswer | null {
  const [answer, setAnswer] = useState<ScenarioClockAnswer | null>(null)
  useEffect(() => {
    if (!live) return
    let on = true
    const load = async () => {
      try {
        const res = await fetch('/api/weather?lat=37.82&lon=-122.48')
        if (!res.ok) return
        const body = (await res.json()) as { scenario?: ScenarioClockAnswer }
        if (on) setAnswer(body.scenario ?? OWN_CLOCK)
      } catch {
        // unreachable for now: keep the last answer, ask again in a minute
      }
    }
    void load()
    const id = window.setInterval(() => void load(), 60_000)
    return () => {
      on = false
      window.clearInterval(id)
    }
  }, [live])
  return answer
}
const OWN_CLOCK: ScenarioClockAnswer = { map: { engine_origin: 0, scenario_origin: 0, rate: 1 }, rate: 1, source: 'wall clock' }

const byUidDetails = (vs: Vehicle[], uid: number) => vs.find((v) => v.uid === uid)


/** Renders nothing. Its effect is the first passive effect of the tree on every commit (it
 *  is App's first child and has no children), so it marks where the effects' work begins. */
function PerfMark() {
  useEffect(() => perfPassiveBegin())
  return null
}

/** Rules 10: the passkey invitation in the address (`#enroll=<operator>.<code>`), read once and
 *  taken out of the address (it works once anyway). */
function readInvite(): { operator: string; code: string } | null {
  const m = /^#enroll=(.+)\.([A-Za-z0-9_-]+)$/.exec(window.location.hash)
  if (!m) return null
  window.history.replaceState(null, '', window.location.pathname + window.location.search)
  return { operator: decodeURIComponent(m[1]), code: m[2] }
}



/** The console. `onMenu`: the launcher's main menu (`make start`), shown over it; absent when
 *  the stack was started without the launcher. */
const NO_INTENTS: Intent[] = []

/** a track's details (TrafficDetails, .vdetails' width and its margin) open on the map's left: a
 *  fit to the track keeps that much clear */
const TRACK_PAD_LEFT = 360

/** Up to this many silent vehicles are drawn with both their estimates, filled. */
const CROWD_SILENT = 10

export default function App({ onMenu }: { onMenu?: () => void }) {
  const { state, status, decisions, setView } = useBridge()
  // who this console acts for (rules 8): asked once the bridge answers, and again on reconnect
  const auth = useAuth()
  // rules 10: a one-time passkey invitation in the address (`fleetmesh-auth passkey-invite`)
  const [invite, setInvite] = useState(readInvite)
  useEffect(() => {
    void refreshAuth()
  }, [])
  useEffect(() => {
    if (status === 'live') void refreshAuth()
  }, [status])
  perfRenderBegin()
  // the bridge's clock (it re-renders the console twice a second), and the world's time on it (bd
  // murmur-iobd.5)
  const now = useBridgeClock(state?.t)
  const scenarioClock = useScenarioClock(status === 'live')
  // every card's time on the bridge's clock said in the same world time as the bar
  useEffect(() => setWorldClock(scenarioClock?.map ?? null), [scenarioClock])
  // rows to scroll to and mark in the fleet list (selected outside it)
  const [revealVeh, setRevealVeh] = useState<Reveal | null>(null)
  const [selected, setSelected] = useState<Set<number>>(new Set())
  /** vehicles selected outside the fleet list: scroll the list to the first and mark it */
  const revealVehicles = useCallback((uids: number[]) => {
    if (uids.length) setRevealVeh({ ids: uids.map(String) })
  }, [])
  const [detailsUid, setDetailsUid] = useState<number | null>(null)
  const [following, setFollowing] = useState(true)
  const [showPlatforms, setShowPlatforms] = useState(false)
  const [focus, setFocus] = useState<{ uid: number; n: number } | null>(null)
  const [draft, setDraft] = useState<Draft>({ mode: 'orbit', center: null, radius: 250, route: [] })
  const [showWeather, setShowWeather] = useState(false)
  // the map: the floating panels keep to it (FloatingPanel.tsx)
  const mapRef = useRef<HTMLElement>(null)
  // the right dock (dock.ts): one drawer at a time over the map's right edge, the top bar's buttons
  // its tabs: the Inbox; the AI watch officer (TaskingPanel.tsx: requests it carries out, never an
  // approval or a decision); rules 5's Queue of orders held for vehicles the link can't reach now; the situation
  // report (Sitrep.tsx: read-only, the wide one); Swarms (docs/DECISIONS.md, "Swarm units")
  const [dock, setDock] = useState<DockId | null>(null)
  // the Parcels list at one place (a parcel mark on the map was clicked), or every parcel; the
  // place lasts while the list is shown (any layout: a drawer, or the Decisions layout's own list)
  const [parcelsAt, setParcelsAt] = useState<string | null>(null)
  const [parcelsShown, setParcelsShown] = useState(false)
  const [parcelsWasShown, setParcelsWasShown] = useState(false)
  if (parcelsShown !== parcelsWasShown) {
    setParcelsWasShown(parcelsShown)
    if (!parcelsShown) setParcelsAt(null)
  }
  const showInbox = dock === 'inbox'
  const showQueue = dock === 'queue'
  const showSitrep = dock === 'sitrep'
  const showParcels = dock === 'parcels'
  const taskingApi = useTasking()
  // the watch officer's drawer shows only where the run has an AI that takes requests
  const officerVisible = officerShown(undefined, taskingApi.reader, taskingApi.status)
  if (dock === 'officer' && !officerVisible) setDock(null)
  const showTasking = dock === 'officer' && officerVisible
  // the message to the officer being written: kept here so it outlives its drawer (switching to the
  // Inbox to decide and back keeps it)
  const [officerDraft, setOfficerDraft] = useState('')
  const [inboxTab, setInboxTab] = useState<InboxTab>('inbox')
  const [inboxSel, setInboxSel] = useState<InboxSelection>(null)
  const [fit, setFit] = useState<{ points: LngLat[]; n: number; padRight: number; padLeft?: number; aloft?: [number, number, number][]; zoom?: number } | null>(null)
  // the drawer shown lies beside the map rather than over its right edge (the layout's choice, set
  // where the map is laid out: `map` below; never in Cockpit): nothing covers the map then
  const dockBeside = useRef(false)
  /** frame these points on the map (MapView's `fit`), `padRight` px kept clear on its right for the
   *  drawer over it (the usual margin when the drawer lies beside the map); `aloft`, points up in
   *  the air that must be in view too */
  const frame = useCallback((points: LngLat[], padRight: number, aloft?: [number, number, number][], padLeft?: number) => {
    const pad = dockBeside.current ? FIT_PAD : padRight
    setFit((f) => ({ points, aloft, n: (f?.n ?? 0) + 1, padRight: pad, padLeft }))
  }, [])
  // predicted link coverage (docs/LINK-COVERAGE.md): the map layer, off by default, and the
  // drafted order's predicted link (the task panel's dry run)
  const [showCoverage, setShowCoverage] = useState(false)
  const [coverageGrid, setCoverageGrid] = useState<CoverageGrid | null>(null)
  const coverageImage = useMemo(() => (showCoverage && coverageGrid ? gridImage(coverageGrid) : null), [showCoverage, coverageGrid])
  // the radio sites the coverage is judged from, marked while the Links layer is on
  const stationLayers = useMemo(
    () => (showCoverage ? buildStationLayers(coverageGrid?.sources.ground_stations ?? []) : []),
    [showCoverage, coverageGrid],
  )
  const [linkRoute, setLinkRoute] = useState<LinkRouteDrawing | null>(null)
  const [mapView, setMapView] = useState<{ center: LngLat; bounds: [number, number, number, number]; zoom: number } | null>(null)
  // bd murmur-vr17: every animated effect at once by default (each its own toggle), and a colour
  // field as an extra, off by default; the regions (weatherField.ts) drawn by the map (weatherMap.ts)
  const [weatherMode] = useState<WeatherMode>('off')
  const [weatherEffects, setWeatherEffects] = useState<Effects>(savedEffects)
  const weatherOn = weatherMode !== 'off' || Object.values(weatherEffects).some(Boolean)
  const weatherField = useWeatherGrid(mapView?.bounds ?? null, weatherOn)
  const weatherView = useMemo<WeatherView | null>(() => {
    const regions = weatherOn ? regionsOf(weatherField.grid) : null
    return regions ? { regions, effects: weatherEffects, field: weatherMode === 'off' ? null : weatherMode } : null
  }, [weatherField.grid, weatherMode, weatherEffects, weatherOn])
  // Escape clears the selection (the Fleet list's Clear), unless a panel that Escape closes is open
  // (the vehicle details, the platforms: that Escape is theirs, and a hundred selected must not go
  // with it) or the key is a text field's or a control's own
  const anySelected = selected.size > 0
  // the traffic layer (docs/design/traffic.md): the track selected (its details open) and the one
  // under the pointer (its predicted path drawn)
  const [trackSel, setTrackSel] = useState<string | null>(null)
  const [trackHover, setTrackHover] = useState<string | null>(null)
  // a hub's details (a click on its mark on the map): its id
  const [hubSel, setHubSel] = useState<string | null>(null)
  const escapeTaken = detailsUid != null || showPlatforms || trackSel != null || hubSel != null
  useEffect(() => {
    if (!anySelected || escapeTaken) return
    const onKey = (e: KeyboardEvent) => {
      const t = e.target as HTMLElement | null
      if (e.key !== 'Escape' || e.defaultPrevented || (t && (t.isContentEditable || ['INPUT', 'TEXTAREA', 'SELECT'].includes(t.tagName)))) return
      setSelected(new Set())
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [anySelected, escapeTaken])
  const onView = useCallback((center: LngLat, bounds: [number, number, number, number], zoom: number) => setMapView({ center, bounds, zoom }), [])

  // the delivery picture (parcels, hubs, drop zones, navigation trust) and which of its layers are drawn:
  // every one by default in a delivery run, none outside one; the Layers menu overrides
  const delivery = useDelivery()
  const [deliveryOn, setDeliveryOn] = useState<Partial<DeliveryDraw>>({})
  const drawn: DeliveryDraw = useMemo(
    () => ({
      hubs: deliveryOn.hubs ?? delivery.active,
      zones: deliveryOn.zones ?? delivery.active,
      parcels: deliveryOn.parcels ?? delivery.active,
      routes: deliveryOn.routes ?? delivery.active,
      areas: deliveryOn.areas ?? delivery.active,
    }),
    [deliveryOn, delivery.active],
  )
  const allVehicles = useMemo(() => state?.vehicles ?? [], [state?.vehicles])
  const vehicles = allVehicles
// rules 30: our vehicles silent, presumed lost, confirmed lost (the review's lists, only in a
  // run under rules 30), and where the keep-out counts each silent one (polled while any is)
  const nodeLoss = useMemo(() => nodeLossView(decisions?.inbox.review), [decisions?.inbox.review])
  const anyStale = useMemo(() => allVehicles.some(isStale), [allVehicles])
  const nodeRows = useNodes(nodeLoss.on && (anyStale || nodeLoss.lost.size > 0 || nodeLoss.review.size > 0))
  // the platform catalog (/api/platforms): endurance and cruise speed for the battery's slot
  const platformCatalog = usePlatformLimits()
  // what the fleet rows' and the map's exception slots are made from (vehicleException.ts)
  const exceptions = useMemo<ExceptionContext>(() => ({ nodes: nodeLoss, platforms: platformCatalog }), [nodeLoss, platformCatalog])
  // the operator's own groups (groups.ts): shared through the bridge, never an order
  const groupsApi = useGroups()
  // the uids in the fleet now, for the groups panel (only while it can be seen)
  const allIntents = decisions?.intents
  // one of ours by its uid as a person names it (its callsign), built on the first call
  const fleetRef = useRef(allVehicles)
  fleetRef.current = allVehicles
  const namer = useRef<{ vehicles: Vehicle[]; name: (uid: string) => string } | null>(null)
  const nameOf = useCallback((uid: string) => {
    const vs = fleetRef.current
    if (namer.current?.vehicles !== vs) namer.current = { vehicles: vs, name: vehicleNamer(vs) }
    return namer.current.name(uid)
  }, [])
  // silent vehicles' estimates (murmur-4y9): the bridge's motion models evaluated every clock
  // tick, so the callback changes identity each tick only while something is silent; none for a
  // vehicle an operator confirmed lost (rules 30: what it last reported is not where it went)
  const silent = useMemo(() => (state ? estimated(vehicles).filter((v) => !nodeLoss.lost.has(v.uid)) : []), [vehicles, state, nodeLoss])
  const estimateTick = silent.length ? Math.floor(now() / 500) : 0
  // in focus for the estimates: selected, or details open (both hypotheses, filled)
  const vehicleFocus = useMemo(() => (detailsUid == null ? selected : new Set([...selected, detailsUid])), [selected, detailsUid])
  // a few silent vehicles (at most CROWD_SILENT): both estimates of each, filled, as before
  const estimateAll = silent.length <= CROWD_SILENT
  const estimateLayers = useCallback<MapLayers>(
    (_screen, plan) => (silent.length ? buildEstimateLayers({ vehicles: silent, nowMs: now(), focus: detailsUid, inFocus: vehicleFocus, showAll: estimateAll, plan }) : []),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [silent, estimateTick, detailsUid, vehicleFocus, estimateAll],
  )
  // rules 30: where each silent vehicle can be as the keep-out counts it, under the estimates: in focus
  const nodeRegionLayers = useMemo(() => (nodeRows.size ? buildNodeRegionLayers(nodeRows.values(), vehicleFocus) : []), [nodeRows, vehicleFocus])
  // ordered versus confirmed (rules 5): queued orders' paths dashed, sent ones not yet
  // confirmed labelled
  const queuedIntents = decisions?.inbox.queued
  const orderTick = Math.floor(now() / 1000)
  const fitPoints = useCallback((points: LngLat[]) => frame(points, fitPad(dock)), [dock, frame])
  /** in to at least `zoom` at these points' middle (a site's mark: its hubs apart) */
  const zoomTo = useCallback((points: LngLat[], zoom: number) => setFit((f) => ({ points, n: (f?.n ?? 0) + 1, padRight: FIT_PAD, zoom })), [])
  const { mem: alertMem, hidden: hiddenAlerts } = useHiddenAlerts(decisions)
  const pending = decisions?.inbox.pending
  const pendingIds = useMemo(() => new Set((pending ?? []).map((i) => i.id)), [pending])
  // The bridge's level of detail follows the view (murmur-and): what the map shows gets full
  // detail, and so do the vehicles the operator is looking at some other way: selected,
  // followed (details open), or named by a pending or queued intent.
  const pins = useMemo(() => {
    const set = new Set<number>(selected)
    if (detailsUid != null) set.add(detailsUid)
    // and the vehicles of orders waiting in the queue (rules 5): their badge, and the
    // re-check when the link is back, are about them
    for (const i of [...(pending ?? []), ...(queuedIntents ?? [])])
      for (const u of i.uids) if (Number.isFinite(Number(u))) set.add(Number(u))
    return [...set].sort((a, b) => a - b)
  }, [selected, detailsUid, pending, queuedIntents])
  useEffect(() => {
    if (mapView) setView({ bounds: mapView.bounds, zoom: mapView.zoom, pin: pins })
  }, [mapView, pins, setView])
  // ---- the fleet by home base (docs/design/fleet-organisation.md): the bridge's view
  // (/api/fleet), or the console's own coarser one from a bridge without it; on the map, the
  // vehicles parked at their own base collapse into its marker (fleetMap.ts)
  const fleetFeed = useFleet()
  const fleetOrders = useMemo(() => (fleetFeed.doc ? null : latestOrders(state?.taskings ?? [], new Map((allIntents ?? []).map((i) => [i.id, i])))), [fleetFeed.doc, state?.taskings, allIntents])
  const fleetAlerts = decisions?.inbox.alerts
  const fleetDoc = useMemo<FleetDoc>(() => {
    if (fleetFeed.doc) return fleetFeed.doc
    const waiting = new Set<number>()
    for (const i of pending ?? []) for (const u of i.uids) if (Number.isFinite(Number(u))) waiting.add(Number(u))
    for (const a of fleetAlerts ?? []) if (a.uid) waiting.add(a.uid)
    // said to be coarse only once the bridge has answered that it has no fleet view
    return { ...fallbackFleet(allVehicles, delivery.places.hubs, fleetOrders ?? new Map(), waiting), derived: fleetFeed.missing }
  }, [fleetFeed.doc, fleetFeed.missing, fleetOrders, allVehicles, delivery.places.hubs, pending, fleetAlerts])
  // the bases opened on the map (a click on a marker), and the zoom's band (sites, bases, every
  // vehicle on its own): the markers change with the band, not with every step of a zoom
  const [openBases, setOpenBases] = useState<Set<string>>(() => new Set())
  const zoomBand = mapView == null ? null : mapView.zoom < SITE_ZOOM ? SITE_ZOOM - 1 : mapView.zoom < EXPAND_ZOOM ? SITE_ZOOM : EXPAND_ZOOM
  const pinSet = useMemo(() => new Set(pins), [pins])
  const baseMap = useMemo(() => {
    const placed = new Set(allVehicles.flatMap((v) => (v.position ? [v.uid] : [])))
    return collapseAtBases(fleetDoc, placed, { keep: pinSet, expanded: openBases, zoom: zoomBand })
  }, [fleetDoc, allVehicles, pinSet, openBases, zoomBand])
  const mapVehicles = useMemo(() => (baseMap.collapsed.size ? allVehicles.filter((v) => !baseMap.collapsed.has(v.uid)) : allVehicles), [allVehicles, baseMap])
  // the hub whose details are open, and what they say (hubSummary.ts)
  const hubSelBase = hubSel ? (fleetDoc.bases.find((b) => b.id === hubSel) ?? null) : null
  const hubSelDetails = useMemo(
    () => (hubSelBase ? hubDetails(hubSelBase, delivery.parcels.parcels, delivery.parcels.elapsed_s, fleetDoc.vehicles.filter((v) => v.base === hubSelBase.id && WITH_OFFICER.test(v.why)).length) : null),
    [hubSelBase, delivery.parcels, fleetDoc.vehicles],
  )
  const selIntent = inboxSel?.kind === 'intent' ? inboxSel.id : null
  const preview = useMemo(
    () => (pending && pending.length ? previewFeatures(pending, vehicles, selIntent) : EMPTY_PREVIEW),
    [pending, vehicles, selIntent],
  )
  const taskings = state?.taskings ?? []
  // the top bar's glance (topbar.ts): ours, and ours in trouble by the rows' own slot
  const glance = useMemo(() => ({ ours: vehicles.length }), [vehicles.length])
  const trouble = useMemo(() => troubleOf(vehicles, exceptions), [vehicles, exceptions])
  // a click on the trouble count shows the next one in trouble in the Fleet list
  const troubleShown = useRef<number | null>(null)
  const showTrouble = useCallback(() => {
    const uid = nextTrouble(trouble.uids, troubleShown.current)
    troubleShown.current = uid
    if (uid != null) setRevealVeh({ ids: [String(uid)] })
  }, [trouble])
  // the delivery layers (deliveryLayers.ts), rebuilt each second and when the feed changes; the
  // suspected interference and link-outage areas come from what vehicles report (suspectAreas.ts)
  const vehiclesRef = useRef(vehicles)
  vehiclesRef.current = vehicles
  const deliveryData = useMemo((): DeliveryData | null => {
    const vs = vehiclesRef.current
    const at = new Map(vs.map((v) => [String(v.uid), v.position]))
    const anyOn = drawn.hubs || drawn.zones || drawn.parcels || drawn.routes || drawn.areas
    if (!anyOn) return null
    const { hubs, drop_zones: zones } = delivery.places
    return {
      hubs,
      zones,
      places: drawn.parcels
        ? parcelPlaces(delivery.parcels.parcels, new Map(hubs.map((h) => [h.id, h])), new Map(zones.map((z) => [z.id, z])), (uid) => at.get(uid) ?? null, delivery.parcels.elapsed_s)
        : [],
      routes: drawn.routes ? deliveryRoutes(state?.taskings ?? [], vs) : [],
      areas: drawn.areas ? suspectAreas(vs, delivery.nav) : [],
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [delivery, drawn, state?.taskings, orderTick])
  // ---- the traffic the fleet does not control (docs/design/traffic.md), as the company's tracker
  // sees it: drawn by how much it matters to the fleet (traffic.ts declutter), never ground truth
  const [trafficToggles, setTrafficToggles] = useState<TrafficToggles>({ on: true, air: true, surface: true, coverage: false, truth: false })
  const setTraffic = useCallback((t: Partial<TrafficToggles>) => setTrafficToggles((o) => ({ ...o, ...t })), [])
  // the traffic bar's feeds, open (its picture button, or a picture alert's link)
  const [trafficFeeds, setTrafficFeeds] = useState(false)
  const showFeeds = useCallback(() => setTrafficFeeds(true), [])
  const traffic = useTraffic(true)
  const picture = traffic.picture
  const pictureRef = useRef(picture)
  pictureRef.current = picture
  const trafficSensors = useTrafficSensors(trafficToggles.on && traffic.available !== false)
  // the simulator's truth: a dev run only, never asked for in a credentials run
  const truthAllowed = auth.auth === 'dev'
  const truthOn = trafficToggles.on && trafficToggles.truth && truthAllowed
  const truthApi = useTrafficTruth(truthOn)
  // what the fleet is, for what matters: read when a new picture comes (once a second)
  const fleetCtx = useMemo<FleetContext>(() => {
    const vs = vehiclesRef.current
    const dom = new Map(vs.map((v) => [v.uid, domainOf(v)]))
    return {
      ours: vs.flatMap((v) => (v.position ? [{ uid: v.uid, at: v.position, altM: v.alt_msl, domain: domainOf(v) }] : [])),
      routes: deliveryRoutes(state?.taskings ?? [], vs).map((r) => ({ uid: r.uid, path: r.path, domain: dom.get(r.uid) ?? 'air' })),
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [picture?.t, state?.taskings])
  // the tracks an alert names, and the one selected: near wherever they are
  const alertTrackIds = useMemo(() => (decisions?.inbox.alerts ?? []).filter((a) => TRAFFIC_ALERT_KINDS.has(a.kind)).flatMap(alertTracks), [decisions?.inbox.alerts])
  const forceKey = [...alertTrackIds, trackSel ?? ''].join()
  // eslint-disable-next-line react-hooks/exhaustive-deps
  const force = useMemo(() => new Set([...alertTrackIds, ...(trackSel ? [trackSel] : [])]), [forceKey])
  const decl = useMemo(() => {
    if (!picture || !trafficToggles.on) return null
    const t0 = performance.now()
    const d = declutter(picture.tracks, fleetCtx, { air: trafficToggles.air, surface: trafficToggles.surface, force })
    perfPush('trafficSort', performance.now() - t0)
    return d
  }, [picture, fleetCtx, trafficToggles.on, trafficToggles.air, trafficToggles.surface, force])
  const trafficTotals = useMemo(() => ({ air: picture?.tracks.filter((t) => t.domain === 'air').length ?? 0, surface: picture?.tracks.filter((t) => t.domain === 'surface').length ?? 0 }), [picture])
  const trackIds = useMemo(() => new Set(picture?.tracks.map((t) => t.id) ?? []), [picture])
  const hasTrack = useCallback((id: string) => trackIds.has(id), [trackIds])
  // a track by the name the map gives it (an alert's subject when it is about no vehicle of ours)
  const trackNameOf = useCallback((id: string) => {
    const t = pictureRef.current?.tracks.find((x) => x.id === id)
    return t ? trackTitle(t) : null
  }, [])
  // a broadcast the company's sensors contradict, joined to where they see it: as the tracker paired
  // them when the bridge passes that on, else the nearest flagged non-cooperative track
  const pairs = useMemo(() => {
    if (!picture || !decl) return []
    const shownIds = new Set(decl.shown.map((r) => r.track.id))
    const byId = new Map(picture.tracks.map((t) => [t.id, t]))
    const list = picture.disagreements
      ? picture.disagreements.flatMap((d) => {
          const b = byId.get(d.track)
          const x = byId.get(d.sensed)
          return b && x ? [{ broadcast: b, sensed: x }] : []
        })
      : disagreementPairs(picture.tracks)
    return list.filter((p) => shownIds.has(p.broadcast.id) && shownIds.has(p.sensed.id))
  }, [picture, decl])
  const truthMatches = useMemo(() => (truthOn && truthApi.truth && picture ? matchTruth(truthApi.truth, picture.tracks) : null), [truthOn, truthApi.truth, picture])
  const trafficInput = useMemo<TrafficLayerInput | null>(
    () => (decl && picture ? { shown: decl.shown, pictureT: picture.t, selected: trackSel, hovered: trackHover, pairs, coverage: trafficToggles.coverage ? trafficSensors : null, truth: truthMatches } : null),
    [decl, picture, trackSel, trackHover, pairs, trafficToggles.coverage, trafficSensors, truthMatches],
  )
  const trackRow = useMemo(() => (trackSel && decl ? (decl.shown.find((r) => r.track.id === trackSel) ?? null) : null), [trackSel, decl])
  // a click on a track selects it (again: no longer); the pointer over one draws its predicted path
  const trafficPick = useMemo(
    () => ({
      layers: TRAFFIC_PICK_LAYERS,
      onPick: (_layer: string, o: unknown) => {
        const id = (o as ShownTrack).track.id
        setTrackSel((s) => (s === id ? null : id))
      },
    }),
    [],
  )
  // a click on a base marker opens its bases (their vehicles drawn one by one), on an open one's
  // closes them again; a click on a track as above
  const trafficOn = trafficInput != null
  // a click on a parcel mark opens the Parcels list at that place
  const parcelsOn = drawn.parcels
  const mapPick = useMemo(
    () => ({
      layers: [...BASE_PICK_LAYERS, ...(parcelsOn ? PARCEL_PICK_LAYERS : []), ...(trafficOn ? TRAFFIC_PICK_LAYERS : [])],
      onPick: (layer: string, o: unknown) => {
        if (BASE_PICK_LAYERS.includes(layer)) {
          // a hub's mark: its details; a site's (zoomed out): in, to its hubs
          const m = o as BaseMarker
          if (m.id.startsWith('site:')) zoomTo(fleetDoc.bases.filter((b) => m.bases.includes(b.id)).map((b) => b.at), SITE_ZOOM + 0.6)
          else setHubSel(m.bases[0])
        }
        else if (PARCEL_PICK_LAYERS.includes(layer)) {
          setParcelsAt((o as ParcelPlace).key)
          setDock('parcels')
        } else trafficPick.onPick(layer, o)
      },
    }),
    [parcelsOn, trafficOn, trafficPick, zoomTo, fleetDoc],
  )
  const onMapHover = useCallback((layer: string | null, object: unknown) => {
    const id = layer && TRAFFIC_PICK_LAYERS.includes(layer) && object ? (object as ShownTrack).track.id : null
    setTrackHover((h) => (h === id ? h : id))
  }, [])
  // the map's layers over the vehicles, in one list (MapView's `layers`)
  const mapLayers = useCallback<MapLayers>(
    (screen, plan) => [
      // other traffic first, under everything of ours
      ...(trafficInput ? buildTrafficLayers(trafficInput, plan, screen) : []),
      ...nodeRegionLayers,
      ...estimateLayers(screen, plan),
      ...buildOrderLayers({ queued: queuedIntents ?? [], intents: allIntents ?? [], taskings: state?.taskings ?? [], vehicles, nowMs: now() }, plan),
      ...stationLayers,
      ...(deliveryData ? buildDeliveryLayers(deliveryData, drawn, plan, screen) : []),
      // the bases' markers, under our vehicles
      ...buildBaseLayers(baseMap.markers, plan),
    ],
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [trafficInput, nodeRegionLayers, estimateLayers, stationLayers, deliveryData, drawn, queuedIntents, allIntents, state?.taskings, vehicles, orderTick, baseMap],
  )
  // every text layer's labels for the one label budget (declutter.ts), with our vehicles' (MapView's):
  // the same data and ticks as the layers above, so a new plan comes with them
  const mapLabels = useCallback(
    (screen: MapScreen) => [
      // the bases' markers first: their room is taken before the hubs' names and the vehicles'
      ...baseLabelCandidates(baseMap.markers, screen),
      ...(silent.length ? estimateLabelCandidates({ vehicles: silent, nowMs: now(), focus: detailsUid, inFocus: vehicleFocus, showAll: estimateAll }, screen) : []),
      ...orderLabelCandidates({ queued: queuedIntents ?? [], intents: allIntents ?? [], taskings: state?.taskings ?? [], vehicles, nowMs: now() }, screen),
      ...(deliveryData ? deliveryLabelCandidates(deliveryData, drawn, screen) : []),
      // other traffic's last: ours keep their room first at equal priority
      ...(trafficInput ? trafficLabelCandidates(trafficInput, screen) : []),
    ],
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [silent, estimateTick, detailsUid, vehicleFocus, estimateAll, queuedIntents, allIntents, state?.taskings, vehicles, orderTick, deliveryData, drawn, trafficInput, baseMap],
  )
  // the full block of a coverage station mark or an estimate under the pointer, while its label is not
  const pictureT = picture?.t ?? 0
  const mapTooltip = useCallback(
    (layer: string, object: unknown) =>
      layer === EXCLUDED_PICK_LAYER
        ? { key: `x:${(object as ExcludedMark).uid}`, text: (object as ExcludedMark).text }
        : BASE_PICK_LAYERS.includes(layer)
          ? { key: `fbt:${(object as BaseMarker).id}`, text: `${(object as BaseMarker).text} · click ${(object as BaseMarker).id.startsWith('site:') ? 'to zoom in to its hubs' : 'for its details'}` }
          : (trafficTooltip(layer, object, (t) => trackTooltip(t, pictureT, nameOf)) ?? deliveryTooltip(layer, object, delivery.parcels.elapsed_s, nameOf) ?? estimateTooltip(layer, object)),
    [delivery.parcels.elapsed_s, nameOf, pictureT],
  )
  // the reference views behind the top bar's one Layers button
  const views: ViewEntry[] = [
    {
      key: 'traffic',
      label: 'Other traffic',
      title: "Aircraft and vessels the fleet does not control, as the company's receivers and sensors see them",
      on: trafficToggles.on && traffic.available !== false,
      off: traffic.available === false ? 'no traffic picture in this run' : null,
      onToggle: () => setTraffic({ on: !trafficToggles.on }),
    },
    { key: 'weather', label: 'Weather', title: 'The weather here and at the selected vehicle, and a colour field on the map', on: showWeather, onToggle: () => setShowWeather((v) => !v) },
    { key: 'links', label: 'Links', title: 'Predicted link coverage on the map: up, degraded and down for a link class at an altitude', on: showCoverage, onToggle: () => setShowCoverage((v) => !v) },
    ...(
      [
        ['hubs', 'Hub areas', "The ring each hub's pads are in (each hub's mark, its state and its details are always shown)"],
        ['zones', 'Drop zones', 'Drop zones, coloured by their latest look: green clear, red occupied or closed, grey not looked at'],
        ['parcels', 'Parcels', 'Parcels where they are, one mark for each place with how many: at a hub, on a vehicle, delivered at a drop zone; a red edge: one there is late. Hover for what they are, click for the list'],
        ['routes', 'Delivery routes', "Each running delivery's route, from the vehicle through its waypoints"],
        ['areas', 'Interference and outages', 'Areas where vehicles report GNSS interference or a link outage: suspected from what they report, not where a source is'],
      ] as const
    ).map(([k, label, title]) => ({
      key: `delivery-${k}`,
      label,
      title,
      on: drawn[k],
      off: delivery.active ? null : 'no delivery run',
      onToggle: () => setDeliveryOn((o) => ({ ...o, [k]: !drawn[k] })),
    })),
    { key: 'platforms', group: 'reference', label: 'Platforms', title: 'The platform catalog: each type, its speeds, endurance and links', on: showPlatforms, onToggle: () => setShowPlatforms((v) => !v) },
  ]

  /** an inbox item was clicked: highlight its vehicles and frame them with the task's preview */
  const focusInbox = useCallback(
    (sel: InboxSelection, uids: number[], intent: Intent | null) => {
      setInboxSel(sel)
      setSelected(new Set(uids))
      revealVehicles(uids)
      const vs = state?.vehicles ?? []
      const pts: LngLat[] = intent ? intentPoints(intent, vs) : uids.map((u) => vs.find((v) => v.uid === u)?.position).filter((p): p is LngLat => p != null)
      if (pts.length) frame(pts, 430)
    },
    [state?.vehicles, revealVehicles, frame],
  )
  /** a traffic alert's link: the track selected, its details open, framed with the vehicle */
  const showTrack = useCallback(
    (id: string, uid: number | null) => {
      const t = pictureRef.current?.tracks.find((x) => x.id === id)
      setTrackSel(id)
      if (!t) return
      setTrafficToggles((o) => ({ ...o, on: true, [t.domain]: true }))
      const pts: LngLat[] = [[t.lon, t.lat]]
      const v = uid != null ? (state?.vehicles ?? []).find((x) => x.uid === uid) : undefined
      if (v?.position) pts.push(v.position)
      // clear of the drawer on the right and of the track's details opening on the left
      frame(pts, 430, undefined, TRACK_PAD_LEFT)
    },
    [state?.vehicles, frame],
  )
  const flyToTrack = useCallback(() => {
    const t = trackRow?.track
    if (t) frame([[t.lon, t.lat]], fitPad(dock), t.domain === 'air' && t.alt_m != null ? [[t.lon, t.lat, t.alt_m]] : undefined, TRACK_PAD_LEFT)
  }, [trackRow, frame, dock])
  const [reviewFocus, setReviewFocus] = useState<{ id: string; n: number } | null>(null)
  /** a link to a review item's card in the Inbox, in view (the person decides there; nothing here
   *  decides) */
  const openReview = useCallback((id: string) => {
    setDock('inbox')
    setInboxTab('inbox')
    setReviewFocus((f) => ({ id, n: (f?.n ?? 0) + 1 }))
  }, [])
  /** the task panel's link (and a line at the head of another drawer, Awaiting.tsx): the pending
   *  intent in the inbox, or a finished one in Decisions */
  const openIntent = useCallback(
    (id: number) => {
      setDock('inbox')
      const i = (pending ?? []).find((x) => x.id === id)
      if (i) {
        setInboxTab('inbox')
        focusInbox({ kind: 'intent', id }, i.uids.map(Number).filter(Number.isFinite), i)
      } else setInboxTab('decisions')
    },
    [pending, focusInbox],
  )
  const toggle = useCallback((uid: number) => {
    setSelected((prev) => {
      const next = new Set(prev)
      if (next.has(uid)) next.delete(uid)
      else next.add(uid)
      return next
    })
  }, [])

  const selectedRef = useRef(selected)
  selectedRef.current = selected
  /** a click on a vehicle in the map or its details: reveal it in the list when this selects it */
  const toggleOutside = useCallback(
    (uid: number) => {
      if (!selectedRef.current.has(uid)) revealVehicles([uid])
      toggle(uid)
    },
    [revealVehicles, toggle],
  )

  // right click: deselect one vehicle, or everything (uid null)
  const deselect = useCallback((uid: number | null) => {
    if (uid == null) setTrackSel(null)
    setSelected((prev) => {
      if (uid == null) return prev.size ? new Set() : prev
      if (!prev.has(uid)) return prev
      const next = new Set(prev)
      next.delete(uid)
      return next
    })
  }, [])

  const onMapClick = useCallback(
    (p: LngLat) => {
      // an orbit's centre or a patrol's waypoint is for the vehicles selected: none, no draft
      if (selected.size === 0) return
      setDraft((d) => (d.mode === 'orbit' ? { ...d, center: p } : d.mode === 'patrol' || d.mode === 'transit' ? { ...d, route: [...d.route, p] } : d))
    },
    [selected],
  )

  // what a layout may do for the person (layouts/parts.ts `ConsoleActs`), each as the console's own
  // controls already do it
  /** one of ours' details open, the map flying there and following it */
  const showVehicle = useCallback((uid: number) => {
    setDetailsUid(uid)
    setFollowing(true)
    setFocus((f) => ({ uid, n: (f?.n ?? 0) + 1 }))
  }, [])
  /** these vehicles selected, the first of them shown in the Fleet list */
  const selectVehicles = useCallback(
    (uids: number[]) => {
      setSelected(new Set(uids))
      revealVehicles(uids)
    },
    [revealVehicles],
  )
  const urgent = useMemo(() => urgentInbox(decisions?.inbox, hiddenAlerts), [decisions?.inbox, hiddenAlerts])
  const lateParcels = useMemo(() => delivery.parcels.parcels.filter((p) => isLate(p, delivery.parcels.elapsed_s)).length, [delivery.parcels])

  // last effect of the tree for this state: children's effects (map, list) have run
  useLayoutEffect(() => perfCommitted(), [state])
  useEffect(() => perfUpdateEnd(), [state])

  // rules 10: a passkey invitation is registered first (no sign-in needed)
  if (auth.auth === 'credentials' && invite) return <EnrollScreen invite={invite} onDone={() => setInvite(null)} />
  // a credentials run: nothing but the sign-in until an operator has signed in
  if (auth.auth === 'credentials' && !auth.signed_in) return <LoginScreen />

  // the head of the drawer the dock shows: the map's banners, then (in any drawer but the Inbox,
  // which holds the cards themselves) every order waiting for a person, so no drawer hides one
  // the top bar's clock: scenario time on the bridge's clock (it re-renders twice a second)
  const clock = clockText(scenarioClock, now())

  // ---- the console's parts (layouts/parts.ts) ------------------------------------------------------
  // Each is built here, once a render, from the state above; the layout `?layout=` chose
  // (layouts/index.ts) only says where each sits. Cockpit.tsx sets them as the console always was.

  // the top bar's pieces
  const bar: BarParts = {
    menu: onMenu ? (
      <button className="ghost menu-btn" onClick={onMenu} title="The main menu: the scenario running, ending it, and the others to start">
        Menu
      </button>
    ) : null,
    brand: <span className="brand">FLEETMESH</span>,
    // the Sitrep's bottom line (topbar.ts): ours, and the one count of ours in trouble (the rows' own
    // slot); quiet while nothing is wrong
    glance: (
      <span className="glance">
        <span title={`${glance.ours} of ours in the fleet`}>
          <b>{glance.ours}</b> vehicles
        </span>
        {trouble.n > 0 && (
          <button className={`trouble ${trouble.tone ?? ''}`} onClick={showTrouble} title={trouble.says} aria-label={trouble.says}>
            {trouble.n} of ours in trouble
          </button>
        )}
      </span>
    ),
    connection:
      status !== 'live' ? (
        <span className={`pill ${status}`} title={status === 'connecting' ? 'Connecting to the bridge' : 'The bridge is not answering: the console is not live'}>
          {status === 'connecting' ? 'CONNECTING' : 'BRIDGE DOWN'}
        </span>
      ) : null,
    // the dock's tabs (dock.ts): one drawer at a time over the map's right edge
    inbox: <InboxButton decisions={decisions} open={showInbox} hiddenAlerts={hiddenAlerts}  now={now} onClick={() => setDock((d) => toggleDock(d, 'inbox'))} />,
    // every drawer button stays on the bar, greyed with the reason when the run has nothing for it
    officer: <TaskingButton api={taskingApi}  onClick={() => setDock((d) => toggleDock(d, 'officer'))}  />,
    // the Queue only while something is held for a link (or its drawer is open)
    queue: <QueueButton decisions={decisions} now={now} open={showQueue} onClick={() => setDock((d) => toggleDock(d, 'queue'))} />,
    // the Sitrep only at group scale (topbar.ts GROUP_SCALE): below it the lists are the report
    sitrep:
      groupScale(vehicles.length) || showSitrep ? (
        <button className={`ghost platforms-btn ix-btn ${showSitrep ? 'on' : ''}`} aria-pressed={showSitrep} onClick={() => setDock((d) => toggleDock(d, 'sitrep'))} title="The situation report: the whole picture, top-down, read-only">
          Sitrep
        </button>
      ) : (
        <OffButton label="Sitrep" off={`for a fleet of more than ${GROUP_SCALE}: below that the lists are the report`} />
      ),
    // the Parcels drawer (Cockpit and Rail; the Decisions layout lists Parcels beside Fleet and Log)
    parcels: (
      <ParcelsButton
        open={showParcels}
        onClick={() => setDock((d) => toggleDock(d, 'parcels'))}
        late={lateParcels}
        total={delivery.parcels.parcels.length}
      />
    ),
    // the operator's load (OperatorLoad.tsx): waiting, the oldest wait, decisions in ten minutes
    load: <OperatorLoad decisions={decisions} now={now} />,
    views: <ViewsMenu entries={views} />,
    operator: <OperatorBadge />,
    clock: (
      <span className="clock" >
        {clock.text}
      </span>
    ),
    // the figures behind the glance, for a layout that words them itself
    figures: { glance, trouble, showTrouble },
  }

  // the head of the drawer the dock shows: every order waiting for a person (in any drawer but the
  // Inbox, which holds the cards themselves), so no drawer hides one
  const awaitRows = dock && headsWithWaiting(dock) ? awaitingRows(pending ?? [], now(), (i) => askSentence({ kind: i.kind, params: i.params as Record<string, unknown>, uids: i.uids, group: i.group }, askFleet(allVehicles))) : []
  const dockHead = awaitRows.length ? <Awaiting rows={awaitRows} onOpen={openIntent} /> : null
  // as the dock's drawer (Cockpit, Rail) it lies over the map's right edge as the others do (`.ix`);
  // the Decisions layout lists it in a frame of its own
  const parcelsPanel = (closable: boolean) => {
    const panel = (
      <ErrorBoundary key="parcels" name="Parcels" className="floating">
        <ParcelsPanel
          feed={delivery}
          nameOf={nameOf}
          onVehicle={showVehicle}
          onPlace={(at) => frame([at], fitPad(dock))}
          onClose={closable ? () => setDock((d) => closeDock(d, 'parcels')) : undefined}
          banners={closable ? dockHead : null}
          place={parcelsAt}
          onAllPlaces={() => setParcelsAt(null)}
          onShown={setParcelsShown}
        />
      </ErrorBoundary>
    )
    return closable ? <section className="ix pc-drawer">{panel}</section> : panel
  }
  // what the message box can put in at its cursor: the selection (one group by name and size, or a
  // few vehicles by callsign)
  const sel = showTasking ? selectionInsert(null, [...selected], (u) => nameOf(String(u))) : null
  const composeInserts: ComposeInsert[] = sel ? [sel.text != null ? { key: 'sel', label: sel.text.length > 24 ? `${selected.size} selected` : sel.text, text: sel.text } : { key: 'sel', label: `${selected.size} selected`, text: null, why: sel.why }] : []
  // the Inbox's content: the dock's drawer while it is the one shown, or wherever a layout keeps it
  const inbox = (
    <ErrorBoundary key="inbox" name="Inbox" className="floating">
      <InboxPanel
        decisions={decisions}
        bases={fleetDoc.bases}
        vehicles={allVehicles}
        now={now}
        tab={inboxTab}
        setTab={setInboxTab}
        selection={inboxSel}
        onFocus={focusInbox}
        onClose={() => setDock((d) => closeDock(d, 'inbox'))}
        mem={alertMem}
        hidden={hiddenAlerts}
        focus={reviewFocus}
        cause={(i) => {
          const link = turnOf(i, taskingApi.items)
          return link ? <TurnCause link={link} onShow={() => setDock('officer')} /> : null
        }}
        taskings={state?.taskings}
        onTrack={showTrack}
        hasTrack={hasTrack}
        trackName={trackNameOf}
        onPicture={showFeeds}
      />
    </ErrorBoundary>
  )
  // the drawer the dock shows (dock.ts), keyed by which it is: a switch mounts the new one afresh
  let drawer: ReactNode = null
  if (showInbox) drawer = inbox
  else if (showQueue)
    drawer = (
      <ErrorBoundary key="queue" name="Queue" className="floating">
        <QueuePanel
          banners={dockHead}
          decisions={decisions}
          vehicles={vehicles}
          now={now}
          onFocus={(uids) => {
            selectVehicles(uids)
            const pts = uids.map((u) => vehicles.find((v) => v.uid === u)?.position).filter((p): p is LngLat => p != null)
            if (pts.length) frame(pts, 430)
          }}
          onClose={() => setDock((d) => closeDock(d, 'queue'))}
        />
      </ErrorBoundary>
    )
  else if (showTasking)
    drawer = (
      <ErrorBoundary key="officer" name="Watch officer" className="floating">
        <TaskingPanel
          api={taskingApi}
          now={now}
          banners={dockHead}
          onClose={() => setDock((d) => closeDock(d, 'officer'))}
          compose={<OfficerCompose api={taskingApi} draft={officerDraft} setDraft={setOfficerDraft} inserts={composeInserts} />}
          groups={groupsApi.groups}
          intents={allIntents ?? NO_INTENTS}
          // an order the officer proposed for a request: its card, in the Inbox while it waits
          onIntent={openIntent}
          // a group the officer made for it: its members selected, nothing sent
          onGroup={selectVehicles}
        />
      </ErrorBoundary>
    )
  else if (showParcels) drawer = parcelsPanel(true)
  else if (showSitrep)
    drawer = (
      <ErrorBoundary key="sitrep" name="Sitrep" className="floating">
        <Sitrep
          banners={dockHead}
          vehicles={vehicles}
          taskings={taskings}
          groups={groupsApi.groups}
          onVehicle={(uid) => {
            // its details, as a click on its fleet row opens them (the map flies there); they
            // open over the map's left edge, under this wide drawer, so the report gives way
            setDock((d) => closeDock(d, 'sitrep'))
            showVehicle(uid)
          }}
          onSelect={selectVehicles}
          onClose={() => setDock((d) => closeDock(d, 'sitrep'))}
        />
      </ErrorBoundary>
    )

  /** The map and everything on it (layouts/parts.ts `map`): what the layout docks over its right
   *  edge, and whether the banners and the Situation chip sit on it. */
  const renderMap = (slot: MapSlot = {}): ReactNode => {
    const docked = Boolean(slot.drawer)
    // the drawer shown over the map (it makes room for it, a fit pads for it) or beside it (frame)
    dockBeside.current = slot.beside ?? (dock != null && !docked)
    return (
      <main ref={mapRef} className={`mapwrap${docked ? ' docked' : ''}`}>
        <ErrorBoundary name="Map" className="floating">
          <MapView
            taskings={taskings}
            vehicles={mapVehicles}
            selected={selected}
            draft={draft}
            focus={focus}
            follow={detailsUid}
            onFollowing={setFollowing}
            preview={preview}
            fit={fit}
            onToggle={toggleOutside}
            onDeselect={deselect}
            onMapClick={onMapClick}
            onView={onView}
            clusters={state?.clusters ?? null}
            layers={mapLayers}
            labels={mapLabels}
            tooltip={mapTooltip}
            exceptions={exceptions}
            linkRoute={linkRoute}
            coverage={coverageImage}
            weather={showCoverage ? null : weatherView}
            pick={mapPick}
            onHover={onMapHover}
          />
        </ErrorBoundary>
        {/* the panels that pop out over the map (FloatingPanel.tsx): each opens on the free spot
            nearest its own corner, clear of the others and of what is fixed on the map, and moves
            by the bar at its top (R20; rule 6) */}
        <FloatLayer kind="map">
          {showCoverage && (
            <FloatingPanel id="links" prefer="bottom-left" reserve={230}>
              <ErrorBoundary name="Link coverage" className="floating">
                <CoverageControl
                  view={mapView}
                  vehicle={selected.size === 1 ? (vehicles.find((v) => selected.has(v.uid)) ?? null) : null}
                  onGrid={setCoverageGrid}
                  onClose={() => setShowCoverage(false)}
                />
              </ErrorBoundary>
            </FloatingPanel>
          )}
          {/* a situation runner's status (make situation): shown only while one has posted it; its
              chip here unless the layout shows it in the bar (bar.run), the open panel here always */}
          {detailsUid != null && byUidDetails(vehicles, detailsUid) && (
            <FloatingPanel id="vehicle-details" prefer="top-left" reserve={480}>
              <ErrorBoundary name="Vehicle details" className="floating">
                <VehicleDetailsWhenFull
                  v={byUidDetails(vehicles, detailsUid)!}
                  taskings={taskings}
                  intents={allIntents}
                  now={now}
                  selected={selected.has(detailsUid)}
                  onToggleSelect={() => toggleOutside(detailsUid)}
                  onClose={() => setDetailsUid(null)}
                  following={following}
                  onFollow={() => {
                    setFollowing(true)
                    setFocus((f) => ({ uid: detailsUid, n: (f?.n ?? 0) + 1 }))
                  }}
                  onReview={openReview}
                  nameOf={nameOf}
                  exceptions={exceptions}
                  nodeLoss={nodeLoss}
                  nodeRow={nodeRows.get(detailsUid)}
                />
              </ErrorBoundary>
            </FloatingPanel>
          )}
          {hubSelBase && hubSelDetails && (
            <FloatingPanel id="hub-details" prefer="top-left" reserve={520}>
              <ErrorBoundary name="Hub details" className="floating">
                <HubDetails
                  d={hubSelDetails}
                  plays={hubSelBase.plays ?? []}
                  weather={hubSelBase.weather ?? null}
                  closed={hubSelBase.open === false}
                  needing={fleetDoc.vehicles.filter((v) => v.base === hubSelBase.id && (v.state === 'attention' || v.state === 'offline'))}
                  nameOf={nameOf}
                  open={openBases.has(hubSelBase.id)}
                  canOpen={openBases.has(hubSelBase.id) || baseMap.markers.some((m) => m.bases.includes(hubSelBase.id) && m.n > 0)}
                  onToggleOpen={() => setOpenBases((o) => toggleExpanded(o, { bases: [hubSelBase.id] }))}
                  onVehicle={showVehicle}
                  onParcels={() => {
                    setParcelsAt(`hub:${hubSelBase.id}`)
                    setDock('parcels')
                  }}
                  actFor={(uid) => {
                    const i = (pending ?? []).find((x) => x.uids.map(Number).includes(uid))
                    if (i) return { label: 'Decide', run: () => openIntent(i.id) }
                    const a = decisions?.inbox.alerts.find((x) => x.uid === uid)
                    if (a)
                      return {
                        label: 'See the alert',
                        run: () => {
                          setDock('inbox')
                          setInboxTab('inbox')
                          focusInbox({ kind: 'alert', id: a.id }, [uid], null)
                        },
                      }
                    return null
                  }}
                  onClose={() => setHubSel(null)}
                />
              </ErrorBoundary>
            </FloatingPanel>
          )}
          {trackRow && picture && (
            <FloatingPanel id="traffic-details" prefer="top-left" reserve={440}>
              <ErrorBoundary name="Traffic details" className="floating">
                <TrafficDetails
                  row={trackRow}
                  pictureT={picture.t}
                  sensors={trafficSensors ?? []}
                  nameOf={nameOf}
                  truth={truthMatches?.find((m) => m.track?.id === trackRow.track.id) ?? null}
                  onFly={flyToTrack}
                  onClose={() => setTrackSel(null)}
                />
              </ErrorBoundary>
            </FloatingPanel>
          )}
          {showPlatforms && (
            <FloatingPanel id="platforms" prefer="top-left" reserve={600}>
              <ErrorBoundary name="Platforms" className="floating">
                <PlatformsPanel
                  onClose={() => setShowPlatforms(false)}
                  onFocus={(uid) => {
                    setSelected(new Set([uid]))
                    revealVehicles([uid])
                    setFocus((f) => ({ uid, n: (f?.n ?? 0) + 1 }))
                    setShowPlatforms(false)
                  }}
                />
              </ErrorBoundary>
            </FloatingPanel>
          )}
        </FloatLayer>
        {/* the map's bars on its bottom left, stacked: other traffic over the weather */}
        <div className="map-legends">
        <ErrorBoundary name="Traffic" className="floating">
          <TrafficBar
            available={traffic.available}
            toggles={trafficToggles}
            set={setTraffic}
            totals={trafficTotals}
            near={decl?.counts.near ?? 0}
            picture={picture?.picture ?? null}
            pictureT={pictureT}
            sensors={trafficSensors}
            truthAllowed={truthAllowed}
            truthAvailable={truthApi.available}
            missed={truthMatches ? truthMatches.filter((m) => !m.track).length : null}
            open={trafficFeeds}
            setOpen={setTrafficFeeds}
          />
        </ErrorBoundary>
        <WeatherLegend mode={weatherMode}
          effects={weatherEffects} onEffects={fx => { setWeatherEffects(fx); saveEffects(fx); setShowCoverage(false) }}
          grid={weatherField.grid} error={showCoverage ? 'Hidden while the Links layer is on' : weatherField.error}
          />
        </div>
        {/* the drawer docked over the map's right edge (Cockpit: the dock's), a direct child: the
            floating panels' area stops at it (floating.ts measureMap) */}
        {slot.drawer}
        {status !== 'live' && (
          <div className="banner" role="status">Lost contact with the bridge: reconnecting</div>
        )}
        {truthOn && status === 'live' && (
          <div className="truth-banner" role="note">
            Simulator truth shown: not available to a real operator
          </div>
        )}
        {PERF_ON && <PerfOverlay />}
      </main>
    )
  }

  const parts: ConsoleParts = {
    bar,
    parcelsPanel: parcelsPanel(false),
    fleet: (
      <ErrorBoundary name="Fleet list" className="slot fleet">
        <FleetPanel vehicles={vehicles} fleet={fleetDoc} selected={selected} onToggle={toggle}
          onDeselect={deselect}
          detailsUid={detailsUid}
          onDetails={(uid) => {
            // a second click on the same row closes it; opening flies the map there
            setDetailsUid((d) => (d === uid ? null : uid))
            if (detailsUid !== uid) {
              setFollowing(true)
              setFocus((f) => ({ uid, n: (f?.n ?? 0) + 1 }))
            }
          }}
          onSelect={(ids) => setSelected(new Set(ids))}
          exceptions={exceptions}
          groups={groupsApi}
          reveal={revealVeh}
          taskings={state?.taskings}
          intents={allIntents}
        />
      </ErrorBoundary>
    ),
    command: (
      <ErrorBoundary name="Task panel" className="slot tasking">
        <TaskPanel vehicles={vehicles} historyVehicles={allVehicles}  selected={selected} draft={draft} setDraft={setDraft} taskings={taskings} pendingIds={pendingIds} onInbox={openIntent} now={now} onLinkRoute={setLinkRoute} intents={allIntents} />
      </ErrorBoundary>
    ),
    map: renderMap,
    dock: { open: dock, drawer, toggle: (id) => setDock((d) => toggleDock(d, id)), show: setDock },
    inbox,
    facts: {
      selected: selected.size,
      urgent,
      drawerOpen: dock != null,
      live: status === 'live',
      reveal: { fleet: revealVeh },
      orders: { taskings, intents: allIntents ?? null },
    },
    act: { showVehicle, selectVehicles, focusInbox, openIntent, openReview, fit: fitPoints },
  }

  // the root: the layout's name on it, so each layout's CSS keeps to its own (layouts/parts.ts)
  return (
    <div className="app" data-layout={LAYOUT}>
      <PerfMark />
      <ShownLayout parts={parts} />
    </div>
  )
}
