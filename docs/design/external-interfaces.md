# External interfaces: what a delivery fleet's command and control must speak

Status: research note and backlog, 2026-11-25. Researched from public sources (section 6); items
marked (unverified) could not be confirmed. Nothing here is built yet. The backlog in section 4 is
ordered by value to the research question; phase 12 in [PLAN.md](../PLAN.md) points here.

## 1. Summary

Every vehicle type talks to three parties: its own control link, the traffic authority for its
medium, and the world around it (other traffic, the city, customers, first responders). In every
domain the vehicle runs itself and a remote person handles exceptions, and identity must be
broadcast. FleetMesh today speaks MAVLink to its vehicles and its own REST, WebSocket and MCP to the
console and agents, and is gaining receivers for ADS-B, AIS and Remote ID
([traffic.md](traffic.md)). The gaps are the identity broadcasts of its own fleet, coordination
with the authorities (UTM, ATC, VTS) and the city and first-responder interfaces. Section 4.1 adds
the control real fleet software exercises that FleetMesh does not yet: fleet and zone pauses,
vehicle-initiated guidance requests, flight termination, and outside instructions as orders to
comply with.

## 2. What real systems speak, by vehicle type

### Delivery drones

- **Control link.**
  - Protocols: MAVLink 2 (ArduPilot, PX4) or a vendor cloud API. DJI's Cloud API uses MQTT, with
    each device described by a capability model of properties, services and events, and docks
    running automated missions. Wing and Zipline are proprietary.
  - Cellular: 3GPP Release 17 adds network authorization of a drone with its UTM service
    supplier, authorization of the controller pairing for command and control, and a CAA-level
    UAV ID.
  - Dedicated spectrum: FCC 47 CFR Part 88 (published January 2025) opens 10 MHz of 5030 to
    5091 MHz for drone control and non-payload links, with a frequency assignment from a dynamic
    frequency management system for each flight.
- **Identity broadcast.**
  - Remote ID is required by 14 CFR Part 89 and specified in ASTM F3411-22a: a broadcast over
    Bluetooth or Wi-Fi, plus a network variant.
  - MAVLink carries it in the OPEN_DRONE_ID message set. ArduPilot implements it (AP_OpenDroneID)
    with an open Remote ID module (ArduRemoteID).
- **Seeing other traffic.**
  - ADS-B In, which ArduPilot's avoidance reads from the ADSB_VEHICLE message.
  - Detect-and-avoid performance per ASTM F3442-25.
  - ACAS sXu (RTCA DO-396) for small uncrewed aircraft.
- **Airspace authority.**
  - Strategic coordination: UTM per ASTM F3548-21. Its roles cover operational intents,
    strategic conflict detection, conformance monitoring, and constraint management and
    processing. InterUSS publishes an open-source DSS, the uss_qualifier test suite and a mock
    USS, runnable in Docker.
  - US BVLOS rule: the proposed Part 108 (NPRM 7 August 2025) adds certified Automated Data
    Service Providers for strategic deconfliction and conformance monitoring, overseen under a
    proposed Part 146. The final rule is pending.
  - Other inputs: LAANC authorization in controlled airspace; NOTAMs and TFRs (the FAA's NOTAM
    Management Service replaced the legacy system in April 2026, and API access is by request);
    weather as METAR and TAF from the aviationweather.gov Data API (no key; JSON, XML or IWXXM).
  - In the EU: U-space under Regulation 2021/664, with certified USSPs and a single CISP.
    Network identification uses ASTM F3411-22a as one means of compliance.
- **Business.** Orders arrive through marketplaces; Wing, for example, delivers inside DoorDash.
  The marketplace API details are unverified.

### Planes on advanced autopilot

- **Real practice.**
  - Reliable Robotics (remotely piloted Cessna Caravan): the remote pilot's push-to-talk voice
    goes by Inmarsat Velaris satellite to the aircraft and is broadcast on the frequency. The
    remote pilot acts on ATC clearances, and the company's C2 link is in certification as a CNPC
    link.
  - Xwing: a ground crew supervises the autonomous flight ("human on the loop").
  - Garmin Emergency Autoland: squawks 7700 and broadcasts automated voice messages on the last
    selected frequency and on 121.5 MHz. It is not aware of other aircraft. Its first real use was
    in December 2025.
- **Control link.**
  - RTCA DO-362A (terrestrial C2 link equipment, 2020) and DO-377A (C2 link system performance,
    2021). RTCA SC-228 is revising them to add use cases including small package delivery.
  - ICAO's C2 link standards (Annex 10 Volume VI) apply from 26 November 2026.
- **ATC.**
  - VHF voice, relayed through the C2 link.
  - CPDLC: US domestic en-route CPDLC needs VDL Mode 2 avionics on the FAA participation list.
  - Mode S transponder and ADS-B Out (1090ES).
  - Squawk codes: 7400 is lost C2 link (in use in the US and UK, with ICAO adoption in progress),
    7600 is radio failure, 7700 is emergency.
  - Flight plans.
  - ACAS Xu (RTCA DO-386, 2020) collision avoidance for larger uncrewed aircraft.

### Autonomous boats and ferries

- **Rules.**
  - The IMO MASS Code (resolution MSC.595(111), adopted May 2026, in effect 1 July 2026) is
    non-mandatory and covers cargo ships. A remote operation centre is certified by the flag
    administration, and a master stays responsible. A mandatory code is targeted for 2030 to
    2032.
  - Passenger ferries such as Zeabuz in Stockholm run under national rules.
- **Onboard data.**
  - The IEC 61162 family: NMEA 0183 (61162-1), NMEA 2000 (61162-3), shipboard Ethernet
    (61162-450, 2024 edition) and its security extension (61162-460).
  - Radar tracked-target output.
- **Identity and other traffic.**
  - AIS Class A and Class B (ITU-R M.1371).
  - VDES is the successor: ITU-R M.2092, with an IMO equipment performance standard adopted May
    2026 (MSC.593(111)).
- **Authority.**
  - Vessel Traffic Service under 33 CFR 161. In San Francisco the station is "San Francisco
    Traffic": inshore sector on channel 14, offshore sector on channel 12, a second radio on
    channel 13 for bridge-to-bridge.
  - VHF channel 16 and DSC for distress.
  - Which small craft must take part in VTS is not verified here.
- **Charts and environment.**
  - S-100 ECDIS has been legal since 1 January 2026.
  - Products: S-101 charts, S-102 bathymetry, S-104 water levels, S-111 surface currents and
    S-124 navigational warnings are in force; S-421 is the route plan exchange.
- **Remote operation centres.**
  - Zeabuz: an onboard supervisor, an onshore supervisor in the ROC and a technical support
    centre. The ROC carries a copy of the bridge interface and 360-degree camera views, and a
    passenger request or an onboard sensor can trigger a takeover.
  - Sea Machines: IP radio, 4G and satellite with failover; a chart-based display with tracked
    targets, video and audio; several vessels from one centre.
- **Schedules.** Ferries publish GTFS and GTFS-Realtime. In the Bay Area it comes through the
  511 SF Bay API, which needs a key; ferry coverage of the realtime vehicle positions is
  unverified.

### Self-driving cars and delivery robots

- **Remote assistance, not remote driving.**
  - Waymo's remote assistance gives advice, not control. The car starts the request and may
    reject the suggestion; about 70 agents cover more than 3,000 vehicles.
  - Remote driving is regulated separately. Germany's AFGBV technical supervisor can approve an
    alternative manoeuvre or deactivate the vehicle, and teleoperated driving got its own
    ordinance (StVFernLV) on 1 December 2025.
- **Video.** Low-latency WebRTC (research shows it adapts slowly to changing 5G conditions) and
  cellular bonding across carriers (DriveU).
- **First responders.** California AB 1777, with DMV rules from 1 July 2026: a two-way link
  answered within 30 seconds, and a fleet must leave an area within 2 minutes of an electronic
  geofencing directive from emergency officials.
- **Roads and city.**
  - C-V2X under the FCC rules effective 11 February 2025 (DSRC ends 14 December 2026), carrying
    SAE J2735 messages: basic safety messages, signal phase and timing, and intersection maps.
  - WZDx work zone feeds (v4.2, with a USDOT feed registry).
  - The Curb Data Specification for loading zones (Curbs, Events and Metrics APIs; Seattle
    defines more than 5,000 loading zones with it).
  - The Mobility Data Specification for reporting to cities. A delivery-robot mode was proposed
    for 2.0 but is thinly supported.
- **Sidewalk robots.** Remote operators watch several robots and take over at street crossings
  (Starship). Coco reports control hand-offs in under 300 ms.

### Patterns across domains

- **Vehicle-agnostic fleet interfaces exist.** VDA 5050 (v2.1 2024, v3.0 2025) uses MQTT topics
  for order, instantActions, state, visualization, connection and factsheet. The DJI Cloud API
  capability model is similar. Either is a template for adding vehicles that do not speak MAVLink.
- **Three layers everywhere:** tactical control on the vehicle, supervision by exception in an
  operations centre, and strategic coordination with an authority (UTM, ATC, VTS or the city).

## 3. What FleetMesh speaks today

- MAVLink 2 to ArduPilot SITL (Copter, Plane, Rover, boat) and to the Rust stand-ins.
- The link emulator: line of sight, mesh, cellular, SATCOM classes, main-to-backup failover.
- GNSS jamming, spoofing and degradation as effects on the fleet's receivers.
- Tidewater: real terrain, tide, current and wind.
- REST, WebSocket and MCP between the bridge, the console and agents.
- In progress ([traffic.md](traffic.md)): ADS-B as SBS-1, AIS as NMEA AIVDM, Remote ID and own
  sensor detections as JSON lines, and the company-side tracker.

## 4. Gaps to fill (backlog, in priority order)

Each item says what FleetMesh would speak, what it would simulate on the other side, and why it
matters to the research question. Items 1 and 2 fit with traffic phase 2.

1. **Identity broadcasts from the fleet.**
   - Speak: Remote ID from our drones (MAVLink OPEN_DRONE_ID, broadcast and network per ASTM
     F3411); ADS-B Out and transponder codes from the plane (7400 on lost C2 link, 7700
     emergency, 7600 radio failure); AIS from our boats (a Class B transponder on a small boat).
   - Simulate: the transmitters, so the traffic simulator and tracker see our fleet as others
     would, and the tracker can tell our own vehicles from everyone else.
   - Why: compliance, and an honest test of the picture.
2. **Strategic deconfliction with other operators (UTM).**
   - Speak: an ASTM F3548 operational intent for every dispatched flight, updated on reroute and
     ended on landing; constraint processing for closures; conformance monitoring, with a
     non-conforming flight becoming an issue.
   - Simulate: run the InterUSS DSS and mock USS (Apache-2.0, Docker), seeded with intents for
     the traffic simulator's other drones.
   - Why: under Part 108 and U-space this is how a delivery operator shares the sky, and the
     conflicts it cannot clear are issues for the person.
3. **Airspace and weather inputs.**
   - Speak: METAR and TAF (aviationweather.gov Data API, or an archive for replay); NOTAMs and
     TFRs; LAANC-style authorization where controlled airspace applies.
   - Simulate: a NOTAM and TFR source with scripted closures, and a recorded or synthetic METAR
     series consistent with tidewater's wind.
   - Why: closures and weather limits are real issue sources.
4. **ATC and VTS counterparts.**
   - Speak: for the plane, ATC clearances and instructions as relayed voice (text in the
     simulation) and as CPDLC-style structured messages; readback, acceptance or "unable"; the
     lost-link procedure. For boats: VTS check-in and sailing plans, and traffic advisories.
   - Simulate: a controller and a VTS operator driven by the traffic simulator's flows.
   - Why: these are consequential decisions a remote person makes in real operations, and they
     belong in the inbox.
5. **Video and remote assistance.**
   - Speak: drop-zone camera stills or a stream for the drop-time look; vehicle-initiated remote
     assistance requests for cars and robots that give advice, not control (confirm a path, pass
     an obstacle, reclassify an object).
   - Simulate: rendered or placeholder frames tied to the scenario's observations, and stuck-car
     cases.
   - Why: what a person sees decides how fast and how well they handle an issue.
6. **City and first-responder interfaces.**
   - Speak: geofencing directives from emergency officials with the 2-minute compliance clock and
     the 30-second two-way contact (AB 1777); Curb Data Specification loading zones and WZDx work
     zones for cars; optionally MDS reporting.
   - Simulate: a responder console that issues directives, and curb and work-zone feeds.
   - Why: these are mandatory, time-critical issue sources for ground vehicles.
7. **Link realism.**
   - Speak: a per-flight frequency assignment on the dedicated 5030 to 5091 MHz band (Part 88,
     with denial possible); the 3GPP authorization step before cellular C2; a voice relay
     channel for the plane (voice over the C2 link); bonded cellular for cars.
   - Simulate: in the link emulator.
   - Why: link authorization failures and voice-relay loss are issues the operator has to handle.
8. **Later.**
   - A VDA 5050-style adapter for vehicles that do not speak MAVLink, so cars, robots or a DJI
     dock can be added.
   - S-104 and S-111 exports from tidewater's tide and current.
   - C-V2X signal phase and timing and intersection maps, if the cars move to real road driving.
   - IEC 61162-450 for real boat hardware.
   - GTFS-Realtime if ferries become fleet nodes rather than background traffic.

### 4.1 Control the fleet software does not yet exercise

Real fleet-operations software controls everything above the vehicle's own reflexes. From the
most common rung to the rarest, the ladder is:

1. **Fleet-level operational control:** whether, where and when the fleet operates. In aviation
   this is the legal authority over initiating, conducting and terminating flights (14 CFR 1.1),
   held by the company. Example: Waymo shrank its San Francisco fleet before a storm and paused
   service there on 18 November 2026.
2. **Tasking:** missions, routes and assignments.
3. **Guidance and approvals:** the vehicle asks and a person advises.
   - Waymo's remote assistance gives advice, not control; the car may reject it.
   - Zoox operators send waypoints that the vehicle validates before following.
   - Cruise had about 1 assistant per 15 to 20 cars.
   - Germany's technical supervisor approves alternative manoeuvres or deactivates the vehicle.
4. **Contingency and emergency commands:** hold, return, land now, abort, contingency landing,
   stop in place, and flight termination (ASTM F3322 parachute systems cut propulsion). Many of
   these also trigger on the vehicle by themselves; Zipline's drones abort a blocked or windy drop
   on their own. The proposed Part 108 flight coordinator intervenes with high-level commands and
   emergency actions, not manual flying (per secondary summaries of the NPRM).
5. **Direct remote control:** exists, but is the exception for an autonomous fleet.
   - DJI live flight controls, Skydio browser flying over 5G, FlytBase joystick mode.
   - Sea Machines' joystick works within about 1 km.
   - Reliable Robotics' remote pilot commands taxi, takeoff and landing through the flight
     management system and a custom panel, with no stick or rudder.
   - Tesla's remote operators take over only at 2 mph or less, as a last resort.
   - Vay's whole service is remote driving, which Germany regulated in December 2025.

Never controlled by the software: the vehicle's inner loops and reflexes (stabilization,
collision-avoidance manoeuvres, lost-link behaviour, onboard aborts), and outside authorities. ATC,
VTS and first responders instruct; the company complies.

FleetMesh covers rung 2 (pickup, deliver, reroute, swap_battery) and rung 4 (hold, return_to_hub,
land_now, abort), and rung 1 only through dispatch. Backlog:

9. **Fleet-level and zone-level controls.**
   - Pause or resume a zone, a hub or the whole fleet; stand down for weather; close a hub.
   - Tie these to automatic triggers (a weather limit forecast, a responder directive).
   - Why: the biggest real decisions in fleet operations happen here, rarely and with large
     effect. They belong in the inbox as consequential orders.
10. **Vehicle-initiated guidance requests (rung 3).**
    - The vehicle asks a question: is this path blocked, may I pass on the left, is the drop
      zone clear in this image? A person answers with advice the vehicle validates and may
      reject.
    - Start with cars and boats; drones at the drop.
    - Why: this is the main way people help autonomous vehicles today, and its attention cost
      scales with the fleet.
11. **Flight termination and transponder emergency codes.**
    - A `terminate` order (motor cut and parachute) only a person can give, on a locked human
      tier.
    - Onboard autonomous triggering for failures.
    - Squawk 7700 and 7400 tied to the plane's emergency and lost-link states.
12. **Outside instructions as orders to comply with.** ATC clearances and instructions, VTS
    directions, responder geofencing directives (with the 2-minute clock). Each enters as a
    constraint the fleet must meet, with compliance logged (items 4 and 6 above simulate the
    sources).
13. **Manual takeover, as an optional research arm only.**
    - Rung 5 with realistic latency and link loss.
    - Off by default, and never the normal path.
    - Why: the most attention-hungry mode there is, and so a useful upper bound to compare the
      other designs against.

## 5. Not worth simulating

Avionics buses (ARINC 429, AFDX), vehicle CAN networks, certification internals (DO-178C,
DO-326A, UNECE R155 and R156) and payment systems. They matter to building a real vehicle, not to
supervising a fleet of them.

## 6. Sources

Drones and UTM:
- [3GPP UAS connectivity](https://www.3gpp.org/news-events/3gpp-news/tsg-ct-work-on-uas-connectivity-identification-tracking)
- [GSMA on 3GPP for UAS](https://www.gsma.com/solutions-and-impact/technologies/internet-of-things/wp-content/uploads/2022/03/Leveraging-3GPP-Cellular-Network-Mechanisms-to-support-UAS-operations-March-2022-002.pdf)
- [FCC UAS spectrum rules](https://www.federalregister.gov/documents/2025/01/08/2024-29967/spectrum-rules-and-policies-for-the-operation-of-unmanned-aircraft-systems)
- [ASTM F3548-21](https://store.astm.org/f3548-21.html)
- [InterUSS monitoring](https://github.com/interuss/monitoring)
- [Part 108 NPRM](https://www.federalregister.gov/documents/2025/08/07/2025-14992/normalizing-unmanned-aircraft-systems-beyond-visual-line-of-sight-operations)
- [DJI Cloud API](https://developer.dji.com/doc/cloud-api-tutorial/en/overview/basic-concept/mqtt.html)
- [MAVLink Open Drone ID](https://mavlink.io/en/services/opendroneid.html)
- [ArduPilot Remote ID](https://ardupilot.org/plane/docs/common-remoteid.html)
- [ASTM F3442-25](https://store.astm.org/f3442-25.html)
- [FAA NOTAM Management Service](https://www.faa.gov/about/initiatives/notam)
- [AviationWeather Data API](https://aviationweather.gov/data/api/)
- [EU U-space regulation](https://eur-lex.europa.eu/eli/reg_impl/2021/664/oj/eng)
- [Wing and DoorDash](https://wing.com/news/wing-and-doordash-expand-drone-delivery)

Aircraft:
- [AOPA on the remotely piloted Caravan](https://www.aopa.org/news-and-media/all-news/2024/march/pilot/future-flight-flying-without-a-pilot)
- [Inmarsat and Reliable Robotics](https://ainonline.com/news-article/2023-03-14/inmarsat-and-reliable-robotics-partner-satellite-communications-remotely)
- [FAA Emergency Autoland notice](https://www.faasafety.gov/SPANS/noticeView.aspx?nid=11667)
- [Garmin Autoland first use](https://www.flightradar24.com/blog/aviation-news/aviation-safety/garmin-emergency-autoland-deployed-first-time/)
- [AIN on Xwing](https://ainonline.com/news-article/2022-10-06/human-loop-my-ride-xwings-self-flying-aircraft)
- [RTCA SC-228 terms of reference](https://www.rtca.org/wp-content/uploads/2026/03/SC-228-TOR-Rev-24_Approved-2026-03-10.pdf)
- [FAA TSO-C213a draft](https://www.faa.gov/sites/faa.gov/files/2022-08/TSO_C213a_Draft_Public_Comment.pdf)
- [UK CAA SSR codes](https://www.caa.co.uk/publication/download/27181)
- [ICAO Annex 10 C2 link](https://www.icao.int/sites/default/files/APAC/Meetings/2020/2020%20SRWG4/4-Information%20Papers/IP03_ICAO%20AI.5%20-%20Annex%2010%20updates%20on%20RPAS%20C2%20Link.pdf)
- [FAA Data Comm](https://www.faa.gov/air_traffic/technology/datacomm)
- [ACAS Xu DO-386](https://www.suasnews.com/2021/01/the-new-acas-xu-mops-do-386-key-takeaways/)

Maritime:
- [IMO MSC 111](https://www.imo.org/en/mediacentre/meetingsummaries/pages/temporary-msc-111.aspx)
- [DNV on the MASS Code](https://www.dnv.com/news/2026/imo-mcs-111-new-mass-code-adopted/)
- [IEC 61162](https://en.wikipedia.org/wiki/IEC_61162)
- [IEC 61162-460:2024](https://webstore.iec.ch/publication/72732)
- [IHO S-100](https://iho.int/en/the-s-100-framework-is-now-operational)
- [Admiralty S-100 timelines](https://www.admiralty.co.uk/s-100-timelines-explained)
- [VTS San Francisco user manual](https://navcen.uscg.gov/sites/default/files/pdf/VTS%20User%20Guides/VTS_SF_UserManual_APR2025.pdf)
- [33 CFR 161](https://www.ecfr.gov/current/title-33/chapter-I/subchapter-P/part-161)
- [ITU-R M.2092](https://www.itu.int/dms_pubrec/itu-r/rec/m/R-REC-M.2092-0-201510-I!!PDF-E.pdf)
- [IMO MSC.593(111)](https://wwwcdn.imo.org/localresources/en/KnowledgeCentre/IndexofIMOResolutions/MSCResolutions/MSC.593(111).pdf)
- [Zeabuz remote supervision](https://newatlas.com/marine/estelle-autonomous-electric-ferry-remote-supervision-trial/?amp=true)
- [Sea Machines](https://sea-machines.com/?p=32)
- [511 transit data](https://511.org/open-data/transit)

Cars and robots:
- [Waymo, advice not control](https://waymo.com/blog/shorts/advice-not-control-the-role-of-remote-assistance/)
- [Waymo fleet response](https://waymo.com/blog/2024/05/fleet-response)
- [California DMV AV regulations](https://www.dmv.ca.gov/portal/news-and-media/new-autonomous-vehicle-regulations-strengthen-oversight-and-enforcement-authorize-trucks-and-transit/)
- [California AV rules in force](https://www.techtimes.com/articles/320155/20260711/california-av-rules-now-active-noncompliance-tickets-two-minute-geofences-million-mile-barrier.htm)
- [Germany AV framework](https://www.hsfkramer.com/insights/2026-10/autonomous-vehicles-in-germany-regulatory-framework-and-key-developments)
- [Germany teledriving](https://www.taylorwessing.com/en/insights-and-events/insights/2026/02/legal-frameworks-for-autonomous-driving-and-teledriving)
- [FCC C-V2X order](https://docs.fcc.gov/public/attachments/FCC-24-123A1_Rcd.pdf)
- [USDOT WZDx](https://www.transportation.gov/av/data/wzdx)
- [Curb Data Specification](https://github.com/openmobilityfoundation/curb-data-specification/wiki/Home)
- [Zoox teleoperations patent](https://patents.google.com/patent/US11307576)
- [Cruise remote assistance](https://www.nbcnewyork.com/news/business/money-report/cruise-confirms-robotaxis-rely-on-human-assistance-every-four-to-five-miles/4836824/)
- [Tesla remote drivers](https://www.planetizen.com/news/2026/04/137261-tesla-reveals-remote-drivers-temporarily-control-its-robotaxis)
- [Vay in Las Vegas](https://electrek.co/2024/01/17/vay-begins-commercial-operation-of-remotely-driven-ev-rides-in-las-vegas/)
- [Waymo November pause](https://kioncentralcoast.com/news/2026/11/18/bay-area-waymo-robotaxi-service-suspended-amid-storm-flooding/)

Control authority (section 4.1):
- [14 CFR 1.1](https://www.law.cornell.edu/cfr/text/14/1.1)
- [Airdata on Part 108](https://airdata.com/blog/part-108)
- [Wing safety](https://wing.com/safety)
- [Zipline remote operations](https://www.zipline.com/blog/future-remote-operations-drones)
- [Zipline aborted deliveries](https://support.flyzipline.com/en/articles/13802569-what-happens-after-a-drone-delivery-is-aborted-and-when-can-i-expect-another-attempt)
- [DJI Dock live flight controls](https://enterprise-insights.dji.com/blog/dji-dock-live-flight-control)
- [FlytBase manual controls](https://docs.flytbase.com/in-flight-modules/how-to-manage-your-flight-operations/how-to-control-your-drone/manual-drone-controls)
- [FlytBase emergency landing](https://docs.flytbase.com/pre-flight-modules/learn-more-about-failsafes/emergency-landing)
- [Skydio browser flying](https://www.skydio.com/blog/skydio-unveils-ability-to-remotely-fly-x10-drone-from-a-browser-anywhere-with-5g-connection)
- [ASTM F3322-24a](https://store.astm.org/f3322-24a.html)
- [Reliable Robotics control center](https://www.aviationpros.com/aircraft-maintenance-technology/aircraft-technology/unmanned/press-release/21213565/reliable-robotics-reliable-robotics-remotely-pilots-aircraft-from-private-control-center-a-first-for-commercial-aviation)
- [IMO autonomous shipping](https://imo.org/en/MediaCentre/HotTopics/Pages/Autonomous-shipping.aspx)
- [Massterly remote operation centre](https://www.wilhelmsen.com/media-news-and-events/news/2024/massterly-launches-pioneering-remote-operation-centre-in-horten)
- [MDS 2.0.0](https://github.com/openmobilityfoundation/mobility-data-specification/releases/tag/2.0.0)
- [VDA 5050 v2.1](https://vda.de/dam/jcr:f0c9c019-1506-4dee-998a-e92723fbf025/VDA5050-V2p1p0.pdf)
- [DriveU](https://techcrunch.com/2022/01/04/driveu-auto-to-power-remote-piloting-of-easymiles-autonomous-shuttles-cocos-sidewalk-robots)
- [Teleoperation over 5G](https://arxiv.org/html/2507.20438v1)
- [Starship and others](https://fortune.com/2021/11/02/robots-roam-some-campuses-and-city-streets-as-contactless-food-delivery-grows/)
