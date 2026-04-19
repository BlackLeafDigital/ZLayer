
# SyncApplyResponse

Response for a real apply. Reports per-resource outcomes, the current commit SHA the sync was applied at (when known), and a short human-readable summary for CLI display.  NOTE: This is a breaking change from the previous dry-run-only `{ diff, message }` shape. Clients inspecting the apply response directly must be updated — callers that only consumed the HTTP status stay working.

## Properties

Name | Type
------------ | -------------
`appliedSha` | string
`results` | [Array&lt;SyncResourceResult&gt;](SyncResourceResult.md)
`summary` | string

## Example

```typescript
import type { SyncApplyResponse } from '@zlayer/client'

// TODO: Update the object below with actual values
const example = {
  "appliedSha": null,
  "results": null,
  "summary": null,
} satisfies SyncApplyResponse

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as SyncApplyResponse
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


