// Display labels for catalog fields (sim/platforms.toml, sim/links.toml), shared by the platform and
// vehicle views. Keys are link classes and profile names.

export const LINKS: Record<string, string> = {
  los: 'Line-of-sight radio',
  mesh: 'Mesh radio',
  cellular: 'Cellular',
  cellular_urban: 'Cellular (urban)',
  cellular_rural: 'Cellular (rural)',
  satcom: 'SATCOM',
  satcom_ku_geo: 'SATCOM (Ku-band GEO)',
  satcom_leo: 'SATCOM (low orbit)',
  satcom_iridium: 'SATCOM (Iridium)',
}

/** SATCOM classes (`space_class`): the satellite service a vehicle's SATCOM terminal uses. */
export const SATCOM_CLASSES: Record<string, string> = {
  inmarsat: 'Inmarsat (geostationary)',
  leo: 'Low orbit',
  iridium: 'Iridium',
}

/** A link profile in words: its label where the catalog has one, else its name. */
export function linkLabel(profile: string | null | undefined): string {
  return profile ? (LINKS[profile] ?? profile) : 'Link'
}
