
# SyncResourceResult

Result of reconciling a single resource during apply.

## Properties

Name | Type
------------ | -------------
`action` | string
`error` | string
`kind` | string
`resource` | string
`status` | string

## Example

```typescript
import type { SyncResourceResult } from '@zlayer/client'

// TODO: Update the object below with actual values
const example = {
  "action": null,
  "error": null,
  "kind": null,
  "resource": null,
  "status": null,
} satisfies SyncResourceResult

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as SyncResourceResult
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


