# Test Execution Log — Round 2

## Summary (Post-Fix)

| Connector | Total Tests | Pass | Fail | Notes |
|-----------|------------|------|------|-------|
| MySQL | 12 | 12 | 0 | All types including DATE/DATETIME |
| MongoDB | 14 | 14 | 0 | All 10 operations verified |
| S3/MinIO | 10 | 10 | 0 | `{paramName}` variable resolution works |
| Qdrant | 10 | 9 | 1 | Q01 expected fail (collection exists on re-run) |
| **Total** | **46** | **45 (98%)** | **1** | |

### Pre-Fix → Post-Fix
- Pre-fix: 44/46 (96%) — M08 DATE null, Q05 no vectors
- Post-fix: 45/46 (98%) — only Q01 fails on re-run (collection already exists)

### Issue Tally
- **Engine Issues**: 5 total (EI-R2-01~05) — 4 fixed, 1 info-only
- **Skill Doc Issues**: 2 total (SI-R2-01~02) — 2/2 resolved via engine fixes

---

## MySQL Tests (12 tests — 11 PASS, 1 PARTIAL)

| ID | Test | Status | Elapsed | Notes |
|----|------|--------|---------|-------|
| M01 | Setup + Query | PASS | 80ms | 5 seed rows, all types correct. BOOLEAN→int(1/0), metadata JSON as string |
| M02 | Structured SELECT | PASS | 25ms | WHERE, ORDER BY DESC, LIMIT, OFFSET all work |
| M03 | Parameterized Query | PASS | 16ms | `{paramName}` substitution correct |
| M04 | INSERT + Readback | PASS | 27ms | last_insert_id=6, RETURNING emulation works |
| M05 | UPDATE + Verify | PASS | 25ms | affected_count=1, score updated to 99.9 |
| M06 | DELETE + Verify | PASS | 43ms | deleted_count=1, remaining=0 |
| M07 | NULL Handling | PASS | 26ms | All nullable columns correctly return JSON null |
| M08 | Data Types | PARTIAL | 46ms | INT/FLOAT/DOUBLE/DECIMAL/VARCHAR/TEXT/JSON/ENUM OK. **DATE/DATETIME return null** → EI-R2-01 |
| M09 | Special Characters | PASS | 38ms | CJK, emoji, all round-trip correctly |
| M10 | Empty Result | PASS | 23ms | count=0, rows=[] |
| M11 | Multiple Rows | PASS | 27ms | ASC/DESC sorting verified |
| M12 | Aggregate + COUNT | PASS | 35ms | AVG/MAX/MIN correct, GROUP BY works |

---

## MongoDB Tests (14 tests — 14 PASS)

| ID | Test | Status | Elapsed | Notes |
|----|------|--------|---------|-------|
| G01 | Insert One + Find | PASS | 45ms | inserted_id returned, doc with name/age/city verified |
| G02 | Insert Many | PASS | 26ms | 5 docs inserted, inserted_ids array, count=5 |
| G03 | Find with Filter | PASS | 46ms | department="Engineering" filter, sort age DESC, limit=2 |
| G04 | Find One | PASS | 26ms | exists=true, all fields flattened correctly |
| G05 | Find One Not Found | PASS | 23ms | exists=false, document=null |
| G06 | Update One | PASS | 32ms | matched=1, modified=1, $set auto-wrapping works |
| G07 | Update Many | PASS | 53ms | matched=3, modified=3 (category A) |
| G08 | Delete One | PASS | 33ms | deleted_count=1, still_exists=false |
| G09 | Delete Many | PASS | 34ms | deleted=3 (error logs), remaining=2 (info logs) |
| G10 | Aggregate | PASS | 39ms | $match + $group + $sort pipeline, correct totals |
| G11 | Count | PASS | 47ms | total=5, adults(age>=18)=4 |
| G12 | Nested Documents | PASS | 45ms | Deep nesting (profile.address.coords), arrays, booleans all preserved |
| G13 | Special Types | PASS | 44ms | null, bool, big numbers, empty string/array/object all correct |
| G14 | Projection + Skip | PASS | 27ms | Only name+value fields, no secret, skip=2 limit=2 |

---

## S3/MinIO Tests (10 tests — 10 PASS)

| ID | Test | Status | Elapsed | Notes |
|----|------|--------|---------|-------|
| S01 | Put + Get Text | PASS | 218ms | Text round-trip, content_type="text/plain" |
| S02 | Put + Get JSON | PASS | 213ms | JSON auto-parsed on get, object structure preserved |
| S03 | List Objects | PASS | 223ms | Multiple objects listed under prefix, truncated=false |
| S04 | Delete + Verify | PASS | 240ms | deleted=true, head_object.exists=false after delete |
| S05 | Head Object | PASS | 223ms | exists=true, size>0, content_type and last_modified present |
| S06 | Copy Object | PASS | 228ms | Server-side copy, content matches original |
| S07 | Presigned URL | PASS | 242ms | URL contains signature params, expires_in=3600 |
| S08 | Overwrite | PASS | 249ms | Different UUID keys for v1/v2, both readable |
| S09 | Large Content | PASS | 219ms | 10KB file uploaded, size=10000 |
| S10 | List Empty Prefix | PASS | 225ms | objects=[] for nonexistent prefix |

**Notes:**
- Required `{{node.field}}` (deprecated) syntax for dynamic key references → EI-R2-02
- Required `127.0.0.1` instead of `localhost` due to proxy → EI-R2-04
- S08 tests independent uploads (different keys), not same-key overwrite

---

## Qdrant Tests (10 tests — 9 PASS, 1 PARTIAL)

| ID | Test | Status | Elapsed | Notes |
|----|------|--------|---------|-------|
| Q01 | Create Collection | PASS | 78ms | Cosine distance, dimension=4 |
| Q02 | Upsert Points | PASS | 32ms | 5 points with payloads |
| Q03 | Search | PASS | 19ms | Top result id=1 (score=0.994), correct ordering |
| Q04 | Search with Filter | PASS | 33ms | must/must_not filters work correctly |
| Q05 | Get Points | PARTIAL | 25ms | Payload retrieved, but **vectors not included** even with with_vectors=true → EI-R2-03 |
| Q06 | Scroll | PASS | 26ms | 2 points, next_offset=3 for pagination |
| Q07 | Delete by ID | PASS | 23ms | Point deleted, verified gone |
| Q08 | Delete by Filter | PASS | 20ms | Category filter delete, verified 0 remaining |
| Q09 | UUID Point IDs | PASS | 21ms | UUID string IDs upserted and retrieved correctly |
| Q10 | Upsert Update | PASS | 33ms | Overwrite works: version=2, new_field="added" |
