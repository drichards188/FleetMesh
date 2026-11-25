// milsymbol (MIT, npm milsymbol) with only what the traffic layer draws: number-based symbol codes
// (MIL-STD-2525D / APP-6D) for air and sea surface. The package's entry adds every standard and
// symbol set (about 840 kB minified); these two are about 110 kB. Its exports map offers only the
// whole, so the parts are imported by path (the package ships src/ for this, see its index.mjs).
// Typed by milsymbolCore.d.ts.
import { ms } from '../node_modules/milsymbol/src/milsymbol.js'
import air from '../node_modules/milsymbol/src/numbersidc/sidc/air.js'
import sea from '../node_modules/milsymbol/src/numbersidc/sidc/sea.js'

ms.addIcons([air, sea])

export default ms
