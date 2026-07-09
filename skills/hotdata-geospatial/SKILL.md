---
name: hotdata-geospatial
description: Use this skill only when the user is working with geospatial data in Hotdata (PostGIS-style SQL like ST_* functions, geometry/WKB, bbox filtering, point-in-polygon, distance/area, lat/lon, spatial joins, “geospatial”, “GIS”, “PostGIS”). Do not load this skill for non-geospatial SQL or general Hotdata usage.
version: 0.13.1
---

# Hotdata Geospatial Skill

Hotdata supports a subset of PostGIS-style functions in **PostgreSQL-dialect SQL**. This skill is data-agnostic — apply it to any table with geometry columns.

**Requires the core `hotdata` skill** for auth, workspace, and table discovery. **Related:** **`hotdata-analytics`** (OLAP SQL), **`hotdata-search`** (BM25/vector).

## Running these queries

All SQL below runs through the core CLI:

```bash
hotdata query "<sql>" [--workspace-id <id>] [--database <db>] [--output table|json|csv]
```

- **Fully qualify tables** as `<connection>.<schema>.<table>` (or `<catalog>.<schema>.<table>` for a managed database) — every `<table>` placeholder below means a qualified name.
- **PostgreSQL dialect:** double-quote any non-lowercase identifier (e.g. `"GeoID"`).
- Discover candidate tables/columns with **`hotdata tables list --connection-id <id>`** (connection tables) or **`hotdata databases tables`** (tables inside a managed database) — see core skill.

---

## Geometry columns

Most geospatial tables in Hotdata carry one or both of:

| Column | Type | Description |
|---|---|---|
| `wkb_geometry` | `Binary` | WKB-encoded geometry (polygon, point, multipolygon, etc.) |
| `wkb_geometry_bbox` | `Struct` | Precomputed bounding box: `xmin`, `ymin`, `xmax`, `ymax` (Float32) |

**Parse `wkb_geometry` with `ST_GeomFromWKB()`** before any spatial function:

```sql
ST_GeomFromWKB(wkb_geometry)
```

**Access `wkb_geometry_bbox` fields with bracket notation** (dot access is not supported):

```sql
wkb_geometry_bbox['xmin']   -- ✓ works
(wkb_geometry_bbox).xmin    -- ✗ not supported
```

Find these columns by their `Binary` / `Struct` types in `hotdata tables list --connection-id <id>`.

---

## Functions

Common building blocks (full catalog, the unsupported set + workarounds, and unit conversions: **[references/functions.md](references/functions.md)**):

- **Construct:** `ST_GeomFromWKB`, `ST_GeomFromText('POLYGON((...))')`, `ST_MakePoint(lon, lat)`
- **Inspect:** `ST_GeometryType`, `ST_IsValid`, `ST_X` / `ST_Y`, `ST_Centroid`, `ST_AsText`
- **Relate (boolean):** `ST_Within`, `ST_Contains`, `ST_Covers`, `ST_Intersects`, `ST_Overlaps`, `ST_Touches`, `ST_Disjoint`, `ST_Equals`
- **Measure (in degrees):** `ST_Distance`, `ST_Length`, `ST_Area`
- **Process:** `ST_ConvexHull`, `ST_Simplify(geom, tol)`, `ST_OrientedEnvelope`

**Key limits** (see reference for the full list + workarounds): no `::geography` cast — **measurements are in decimal degrees**, convert with the factors in the reference (distance `× 111000` ≈ meters at mid-latitudes; **area is only order-of-magnitude**). No `ST_Buffer`, `ST_DWithin`, `ST_MakeEnvelope`, `ST_Union`, `ST_Transform`, or GeoJSON I/O — use the documented substitutes.

---

## Common patterns

### Geometry types in a table

```sql
SELECT ST_GeometryType(ST_GeomFromWKB(wkb_geometry)) AS geom_type, COUNT(*)
FROM <table>
WHERE wkb_geometry IS NOT NULL
GROUP BY 1
```

### Bounding-box filter (replaces ST_MakeEnvelope / ST_DWithin)

Use `ST_GeomFromText` with a **closed** WKT polygon ring (repeat the first vertex):

```sql
WHERE ST_Within(
  ST_Centroid(ST_GeomFromWKB(wkb_geometry)),
  ST_GeomFromText('POLYGON((minLon minLat, maxLon minLat, maxLon maxLat, minLon maxLat, minLon minLat))')
)
```

**Faster** on large tables — filter the precomputed bbox struct (no WKB parsing); use the `ST_Within` form when you need centroid-in-polygon precision:

```sql
WHERE wkb_geometry_bbox['xmin'] >= <minLon>
  AND wkb_geometry_bbox['xmax'] <= <maxLon>
  AND wkb_geometry_bbox['ymin'] >= <minLat>
  AND wkb_geometry_bbox['ymax'] <= <maxLat>
```

### Point-in-polygon

```sql
SELECT *
FROM <table>
WHERE ST_Contains(ST_GeomFromWKB(wkb_geometry), ST_MakePoint(<lon>, <lat>))
```

### Nearest neighbors (closest N to a point)

```sql
SELECT <id_col>,
  ST_Distance(ST_Centroid(ST_GeomFromWKB(wkb_geometry)), ST_MakePoint(<lon>, <lat>)) * 111000 AS dist_meters
FROM <table>
WHERE wkb_geometry IS NOT NULL
ORDER BY dist_meters
LIMIT 10
```

### Distance between two points

```sql
SELECT
  ST_Distance(ST_MakePoint(<lon1>, <lat1>), ST_MakePoint(<lon2>, <lat2>)) * 111000 AS dist_meters,
  ST_Distance(ST_MakePoint(<lon1>, <lat1>), ST_MakePoint(<lon2>, <lat2>)) * 69.0   AS dist_miles
```

### Area of polygons (order-of-magnitude — see reference caveat)

```sql
SELECT <id_col>,
  ST_Area(ST_GeomFromWKB(wkb_geometry)) * 111000 * 111000        AS area_sqm,
  ST_Area(ST_GeomFromWKB(wkb_geometry)) * 111000 * 111000 / 4047 AS area_acres
FROM <table>
WHERE wkb_geometry IS NOT NULL
```

### Centroid coordinates

```sql
SELECT <id_col>,
  ST_X(ST_Centroid(ST_GeomFromWKB(wkb_geometry))) AS lon,
  ST_Y(ST_Centroid(ST_GeomFromWKB(wkb_geometry))) AS lat
FROM <table>
WHERE wkb_geometry IS NOT NULL
```

### Export / simplify as WKT

```sql
-- raw WKT
SELECT <id_col>, ST_AsText(ST_GeomFromWKB(wkb_geometry)) AS wkt FROM <table> WHERE wkb_geometry IS NOT NULL LIMIT 10
-- simplified (tolerance in degrees, ~11 m at mid-latitudes)
SELECT <id_col>, ST_AsText(ST_Simplify(ST_GeomFromWKB(wkb_geometry), 0.0001)) AS wkt FROM <table> WHERE wkb_geometry IS NOT NULL
```

---

## Workflow: explore a new geospatial table

1. **Find geometry columns** — `hotdata tables list --connection-id <id>`; look for `Binary` (WKB) / `Struct` (bbox) types.
2. **Geometry types** — run the "Geometry types in a table" pattern above.
3. **Coverage / extent** — aggregate the bbox struct:
   ```sql
   SELECT MIN(wkb_geometry_bbox['xmin']) AS min_lon, MIN(wkb_geometry_bbox['ymin']) AS min_lat,
          MAX(wkb_geometry_bbox['xmax']) AS max_lon, MAX(wkb_geometry_bbox['ymax']) AS max_lat
   FROM <table> WHERE wkb_geometry_bbox IS NOT NULL
   ```
4. **Sample WKT** — run the "Export as WKT" pattern with `LIMIT 3` to see geometry structure.
