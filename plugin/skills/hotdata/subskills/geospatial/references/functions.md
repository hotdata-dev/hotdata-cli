# Geospatial function reference

Supported PostGIS-style functions, the unsupported set with workarounds, and degree→unit conversion factors. Hotdata uses **PostgreSQL-dialect SQL**; run everything through `hotdata query` (see the skill's "Running these queries"). Parse `wkb_geometry` with `ST_GeomFromWKB()` before passing it to any function.

## Supported functions

### Input / construction

| Function | Example |
|---|---|
| `ST_GeomFromWKB(col)` | `ST_GeomFromWKB(wkb_geometry)` |
| `ST_GeomFromText(wkt)` | `ST_GeomFromText('POLYGON((...))')` |
| `ST_MakePoint(lon, lat)` | `ST_MakePoint(-122.27, 37.80)` |

### Output

| Function | Example |
|---|---|
| `ST_AsText(geom)` | `ST_AsText(ST_GeomFromWKB(wkb_geometry))` → WKT string |
| `ST_AsBinary(geom)` | `ST_AsBinary(ST_GeomFromWKB(wkb_geometry))` → WKB binary |

### Accessors / inspection

| Function | Returns |
|---|---|
| `ST_GeometryType(geom)` | e.g. `ST_Polygon`, `ST_MultiPolygon`, `ST_Point` |
| `ST_IsValid(geom)` | boolean |
| `ST_NumPoints(geom)` | integer |
| `ST_NPoints(geom)` | integer (alias for `ST_NumPoints`) |
| `ST_X(point)` | longitude (float) |
| `ST_Y(point)` | latitude (float) |
| `ST_Centroid(geom)` | point geometry |

### Measurement

All results are in **decimal degrees** (no `::geography` / meter-native support — see conversions below).

| Function | Unit | Notes |
|---|---|---|
| `ST_Area(geom)` | degrees² | rough — see area caveat in conversions |
| `ST_Length(geom)` | degrees | `× 111000` ≈ meters (mid-latitude) |
| `ST_Distance(geom_a, geom_b)` | degrees | `× 111000` ≈ meters (mid-latitude) |

### Spatial relationships (all return `boolean`)

| Function | Meaning |
|---|---|
| `ST_Within(a, b)` | `a` is completely inside `b` |
| `ST_Contains(a, b)` | `a` contains `b` |
| `ST_Covers(a, b)` | `a` covers `b` (includes boundary) |
| `ST_CoveredBy(a, b)` | `a` is covered by `b` |
| `ST_Intersects(a, b)` | geometries share any space |
| `ST_Overlaps(a, b)` | geometries overlap (same dimension) |
| `ST_Touches(a, b)` | share boundary only, no interior overlap |
| `ST_Crosses(a, b)` | geometries cross (different dimensions) |
| `ST_Disjoint(a, b)` | geometries share no space |
| `ST_Equals(a, b)` | geometries are spatially identical |

### Processing / geometry operations

| Function | Notes |
|---|---|
| `ST_ConvexHull(geom)` | returns convex hull polygon |
| `ST_Simplify(geom, tolerance)` | Douglas–Peucker; tolerance in degrees (~11 m at mid-latitudes) |
| `ST_OrientedEnvelope(geom)` | minimum oriented bounding box |

## Not supported (with workarounds)

| Category | Not supported | Workaround |
|---|---|---|
| Output | `ST_AsGeoJSON`, `ST_AsEWKT` | use `ST_AsText`; parse WKT client-side |
| Cast | `::geography` | multiply degrees by ~111,000 for meters |
| Input | `ST_MakeEnvelope`, `ST_GeomFromGeoJSON`, `ST_MakeLine` | use `ST_GeomFromText('POLYGON(...)')` for envelopes |
| Accessors | `ST_SRID`, `ST_IsEmpty`, `ST_NumGeometries`, `ST_GeometryN`, `ST_ExteriorRing`, `ST_PointN`, `ST_StartPoint`, `ST_EndPoint` | — |
| Measurement | `ST_Perimeter`, `ST_MaxDistance` | — |
| Relationships | `ST_DWithin` | use `ST_Within` + `ST_GeomFromText('POLYGON(...)')` |
| Processing | `ST_Buffer`, `ST_Envelope`, `ST_Boundary`, `ST_Union`, `ST_Intersection`, `ST_Difference`, `ST_SymDifference`, `ST_Collect`, `ST_ClosestPoint`, `ST_Snap`, `ST_BoundingDiagonal`, `ST_Expand` | use `ST_OrientedEnvelope` instead of `ST_Envelope` |
| Projection | `ST_Transform`, `ST_SetSRID`, `ST_FlipCoordinates` | — |

## Degree → unit conversions

All measurements come back in decimal degrees. Convert with these factors — **approximations** that assume ~37°N and ignore longitude shrink (`cos(latitude)`); accuracy drops toward the poles.

| To get | Multiply degrees by |
|---|---|
| Meters (distance) | × 111,000 |
| Kilometers (distance) | × 111 |
| Miles (distance) | × 69.0 |
| Feet (distance) | × 364,173 |
| m² (area) | × 111,000² = × 12,321,000,000 |
| ft² (area) | × 111,000² × 10.7639 |
| Acres (area) | × 111,000² ÷ 4,047 |

> **Area is especially rough.** Distance error grows with latitude; area error grows with its *square* because the `111000²` factor ignores the `cos(latitude)` longitude shrink entirely. Treat square-meter/acre figures as order-of-magnitude, not precise — re-project externally when you need accurate area.
