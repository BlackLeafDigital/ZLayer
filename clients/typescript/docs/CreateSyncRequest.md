
# CreateSyncRequest

Body for `POST /api/v1/syncs`.

## Properties

Name | Type
------------ | -------------
`autoApply` | boolean
`deleteMissing` | boolean
`gitPath` | string
`name` | string
`projectId` | string

## Example

```typescript
import type { CreateSyncRequest } from '@zlayer/client'

// TODO: Update the object below with actual values
const example = {
  "autoApply": null,
  "deleteMissing": null,
  "gitPath": null,
  "name": null,
  "projectId": null,
} satisfies CreateSyncRequest

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as CreateSyncRequest
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


