---
name: hotdata-geospatial
description: Use this skill only when the user is working with geospatial data in Hotdata (PostGIS-style SQL like ST_* functions, geometry/WKB, bbox filtering, point-in-polygon, distance/area, lat/lon, spatial joins, “geospatial”, “GIS”, “PostGIS”). Do not load this skill for non-geospatial SQL or general Hotdata usage.
version: 0.1.14
---

# Hotdata Geospatial Skill

Use this skill when working with geospatial data in Hotdata. Hotdata supports a subset of PostGIS-style functions using **PostgreSQL dialect SQL**. This reference is dataset-agnostic — apply it to any table with geometry columns.

---

## Geometry Columns

Most geospatial datasets in Hotdata carry one or both of:

| Column | Type | Description |
|---|---|---|
| `wkb_geometry` | `Binary` | WKB-encoded geometry (polygon, point, multipolygon, etc.) |
| `wkb_geometry_bbox` | `Struct` | Precomputed bounding box with fields `xmin`, `ymin`, `xmax`, `ymax` (Float32) |

**Always parse `wkb_geometry` with `ST_GeomFromWKB()` before using it in any spatial function:**

```sql
ST_GeomFromWKB(wkb_geometry)
```

**Access `wkb_geometry_bbox` fields with bracket notation** (dot access is not supported):

```sql
wkb_geometry_bbox['xmin']   -- ✓ works
(wkb_geometry_bbox).xmin    -- ✗ not supported
```

Discover geometry columns with:

```sql
hotdata tables list --connection-id <id>
```

---

## Supported Functions

### Input / Construction

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

### Accessors / Inspection

| Function | Returns |
|---|---|
| `ST_GeometryType(geom)` | e.g. `ST_Polygon`, `ST_MultiPolygon`, `ST_Point` |
| `ST_IsValid(geom)` | boolean |
| `ST_NumPoints(geom)` | integer |
| `ST_NPoints(geom)` | integer (alias for ST_NumPoints) |
| `ST_X(point)` | longitude (float) |
| `ST_Y(point)` | latitude (float) |
| `ST_Centroid(geom)` | point geometry |

### Measurement

| Function | Unit | Notes |
|---|---|---|
| `ST_Area(geom)` | degrees² | Multiply by `111000 * 111000` for m², then `* 10.7639` for ft² |
| `ST_Length(geom)` | degrees | Multiply by `111000` for approximate meters |
| `ST_Distance(geom_a, geom_b)` | degrees | Multiply by `111000` for approximate meters |

> **No meter-native measurements:** `::geography` cast is not supported. All measurements are in decimal degrees. The conversion factor ~111,000 m/degree is accurate at mid-latitudes (~30–50°N/S) and degrades toward the poles.

### Spatial Relationships

All return `boolean`:

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

### Processing / Geometry Operations

| Function | Notes |
|---|---|
| `ST_ConvexHull(geom)` | Returns convex hull polygon |
| `ST_Simplify(geom, tolerance)` | Douglas-Peucker simplification; tolerance in degrees |
| `ST_OrientedEnvelope(geom)` | Minimum oriented bounding box |

---

## Not Supported

| Category | Not Supported | Workaround |
|---|---|---|
| Output | `ST_AsGeoJSON`, `ST_AsEWKT` | Use `ST_AsText`; parse WKT client-side |
| Cast | `::geography` | Multiply degrees by ~111,000 for meters |
| Input | `ST_MakeEnvelope`, `ST_GeomFromGeoJSON`, `ST_MakeLine` | Use `ST_GeomFromText('POLYGON(...)')` for envelopes |
| Accessors | `ST_SRID`, `ST_IsEmpty`, `ST_NumGeometries`, `ST_GeometryN`, `ST_ExteriorRing`, `ST_PointN`, `ST_StartPoint`, `ST_EndPoint` | — |
| Measurement | `ST_Perimeter`, `ST_MaxDistance` | — |
| Relationships | `ST_DWithin` | Use `ST_Within` + `ST_GeomFromText('POLYGON(...)')` |
| Processing | `ST_Buffer`, `ST_Envelope`, `ST_Boundary`, `ST_Union`, `ST_Intersection`, `ST_Difference`, `ST_SymDifference`, `ST_Collect`, `ST_ClosestPoint`, `ST_Snap`, `ST_BoundingDiagonal`, `ST_Expand` | Use `ST_OrientedEnvelope` instead of `ST_Envelope` |
| Projection | `ST_Transform`, `ST_SetSRID`, `ST_FlipCoordinates` | — |

---

## Common Patterns

### Check geometry types in a table

```sql
SELECT ST_GeometryType(ST_GeomFromWKB(wkb_geometry)) AS geom_type, COUNT(*)
FROM <table>
WHERE wkb_geometry IS NOT NULL
GROUP BY 1
```

### Bounding box filter (replaces ST_MakeEnvelope / ST_DWithin)

Use `ST_GeomFromText` with a closed WKT polygon ring:

```sql
WHERE ST_Within(
  ST_Centroid(ST_GeomFromWKB(wkb_geometry)),
  ST_GeomFromText('POLYGON((minLon minLat, maxLon minLat, maxLon maxLat, minLon maxLat, minLon minLat))')
)
```

**Vertex order:** `(minLon minLat, maxLon minLat, maxLon maxLat, minLon maxLat, minLon minLat)` — close the ring by repeating the first point.

**Faster alternative** using the precomputed bbox struct (no WKB parsing):

```sql
WHERE wkb_geometry_bbox['xmin'] >= <minLon>
  AND wkb_geometry_bbox['xmax'] <= <maxLon>
  AND wkb_geometry_bbox['ymin'] >= <minLat>
  AND wkb_geometry_bbox['ymax'] <= <maxLat>
```

Use the bbox approach for large tables where WKB parsing is expensive; use `ST_Within` when you need centroid-in-polygon precision.

### Point-in-polygon test

```sql
SELECT *
FROM <table>
WHERE ST_Contains(
  ST_GeomFromWKB(wkb_geometry),
  ST_MakePoint(<lon>, <lat>)
)
```

### Nearest neighbors (closest N features to a point)

```sql
SELECT
  <id_col>,
  ST_Distance(
    ST_Centroid(ST_GeomFromWKB(wkb_geometry)),
    ST_MakePoint(<lon>, <lat>)
  ) * 111000 AS dist_meters
FROM <table>
WHERE wkb_geometry IS NOT NULL
ORDER BY dist_meters
LIMIT 10
```

### Distance between two known points

```sql
SELECT
  ST_Distance(ST_MakePoint(<lon1>, <lat1>), ST_MakePoint(<lon2>, <lat2>)) * 111000 AS dist_meters,
  ST_Distance(ST_MakePoint(<lon1>, <lat1>), ST_MakePoint(<lon2>, <lat2>)) * 69.0   AS dist_miles
```

### Area of polygon features

```sql
SELECT
  <id_col>,
  ST_Area(ST_GeomFromWKB(wkb_geometry)) * 111000 * 111000            AS area_sqm,
  ST_Area(ST_GeomFromWKB(wkb_geometry)) * 111000 * 111000 * 10.7639 AS area_sqft,
  ST_Area(ST_GeomFromWKB(wkb_geometry)) * 111000 * 111000 / 4047     AS area_acres
FROM <table>
WHERE wkb_geometry IS NOT NULL
```

### Centroid coordinates

```sql
SELECT
  <id_col>,
  ST_X(ST_Centroid(ST_GeomFromWKB(wkb_geometry))) AS lon,
  ST_Y(ST_Centroid(ST_GeomFromWKB(wkb_geometry))) AS lat
FROM <table>
WHERE wkb_geometry IS NOT NULL
```

### Convert to WKT for export or inspection

```sql
SELECT <id_col>, ST_AsText(ST_GeomFromWKB(wkb_geometry)) AS wkt
FROM <table>
WHERE wkb_geometry IS NOT NULL
LIMIT 10
```

### Simplify geometry for faster rendering

```sql
SELECT <id_col>, ST_AsText(ST_Simplify(ST_GeomFromWKB(wkb_geometry), 0.0001)) AS simplified_wkt
FROM <table>
WHERE wkb_geometry IS NOT NULL
```

Tolerance is in degrees (~11 m at mid-latitudes). Increase for coarser simplification, decrease for finer.

---

## Unit Conversion Reference

| To get | Multiply degrees by |
|---|---|
| Meters (distance) | × 111,000 |
| Kilometers (distance) | × 111 |
| Miles (distance) | × 69.0 |
| Feet (distance) | × 364,173 |
| m² (area) | × 111,000² = × 12,321,000,000 |
| ft² (area) | × 111,000² × 10.7639 |
| Acres (area) | × 111,000² ÷ 4,047 |

> These conversions assume ~37°N latitude. They are approximations — accuracy decreases significantly above 60°N or below 60°S.

---

## Workflow: Exploring a New Geospatial Dataset

1. **Check for geometry columns:**
   ```
   hotdata tables list --connection-id <id>
   ```
   Look for `Binary` (WKB) or `Struct` (bbox) typed columns.

2. **Verify geometry types:**
   ```sql
   SELECT ST_GeometryType(ST_GeomFromWKB(wkb_geometry)) AS type, COUNT(*)
   FROM <table> WHERE wkb_geometry IS NOT NULL GROUP BY 1
   ```

3. **Check coverage (bounding box of entire dataset):**
   ```sql
   SELECT
     MIN(wkb_geometry_bbox['xmin']) AS min_lon,
     MIN(wkb_geometry_bbox['ymin']) AS min_lat,
     MAX(wkb_geometry_bbox['xmax']) AS max_lon,
     MAX(wkb_geometry_bbox['ymax']) AS max_lat
   FROM <table>
   WHERE wkb_geometry_bbox IS NOT NULL
   ```

4. **Sample WKT to understand geometry structure:**
   ```sql
   SELECT ST_AsText(ST_GeomFromWKB(wkb_geometry)) FROM <table>
   WHERE wkb_geometry IS NOT NULL LIMIT 3
   ```
