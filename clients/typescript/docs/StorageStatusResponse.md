
# StorageStatusResponse

Storage status response

## Properties

Name | Type
------------ | -------------
`replication` | [ReplicationInfo](ReplicationInfo.md)

## Example

```typescript
import type { StorageStatusResponse } from '@zlayer/client'

// TODO: Update the object below with actual values
const example = {
  "replication": null,
} satisfies StorageStatusResponse

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as StorageStatusResponse
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


