# CEL contract for complete event exports

The shim exposes Envoy's completed-request host properties to CEL with their Envoy types:

| Property | CEL type | Envoy encoding |
| --- | --- | --- |
| `request.time` | `timestamp` | Signed 64-bit Unix nanoseconds |
| `request.duration` | `duration` | Signed 64-bit nanoseconds |
| `response.code` | `int` | Signed 64-bit integer |
| `response.size` | `int` | Signed 64-bit integer |

The scalar values are the eight-byte payloads returned by Envoy's Proxy-Wasm property API. The shim decodes them as little-endian signed values, matching its existing timestamp and integer handling.

## Conversion functions

These generic CEL functions are registered for every expression:

- `isPresent(value) -> bool`: false for CEL `null`, true for any non-null value. Use it to guard optional Envoy host properties and optional stored values. CEL's `has(field)` macro checks map-key presence, so it is not a substitute when the shim represents an absent property as `null`.
- `timestampToEpochMillis(timestamp) -> int`: Unix epoch milliseconds.
- `durationToMillis(duration) -> int`: whole milliseconds; fractional milliseconds are truncated.
- `intToString(int) -> string`: decimal rendering for fields whose schema requires a string.

Each function requires exactly one argument. The conversion functions require the stated type; `isPresent` accepts any CEL value. CEL evaluation returns a function error for an incorrect argument count or conversion type.

Use the functions and typed properties directly in the operator's event expression:

```cel
isPresent(request.time) ? timestampToEpochMillis(request.time) : 0
isPresent(request.duration) ? durationToMillis(request.duration) : 0
isPresent(response.code) ? response.code : 0
isPresent(response.size) ? response.size : 0
intToString(isPresent(response.code) ? response.code : 0)
```

For an event start and logged time:

```cel
'time': isPresent(request.time) ? timestampToEpochMillis(request.time) : 0,
'logged_time': (isPresent(request.time) ? timestampToEpochMillis(request.time) : 0) +
    (isPresent(request.duration) ? durationToMillis(request.duration) : 0)
```

Use the response properties without casts for numeric `code` and `body_length` fields. `int()` is unnecessary. The timestamp and duration values are not protobuf messages, so CEL methods such as `getSeconds()` and `getMilliseconds()` are unavailable.

## Export configuration

Configure the complete event as one `StoreAction`, with `exportToHost: true` and `exportFormat: "json"`. For example, its final action can include:

```json
{
  "type": "store",
  "predicate": "true",
  "terminal": false,
  "path": "ocsf.event",
  "exportToHost": true,
  "exportFormat": "json",
  "value": "{ 'time': isPresent(request.time) ? timestampToEpochMillis(request.time) : 0, 'http_response': { 'code': isPresent(response.code) ? response.code : 0, 'body_length': isPresent(response.size) ? response.size : 0, 'latency': isPresent(request.duration) ? durationToMillis(request.duration) : 0 }, 'status_code': intToString(isPresent(response.code) ? response.code : 0) }"
}
```

The filter-state property is `wasm.kuadrant.ocsf.event`; Envoy can write it as a raw access-log line with `%FILTER_STATE(wasm.kuadrant.ocsf.event:PLAIN)%`. The shim implements generic CEL conversions and JSON serialization; the operator remains responsible for OCSF fields and schema.
