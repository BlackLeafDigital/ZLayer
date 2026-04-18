
# ReplicationInfo

Replication detail within the storage status response

## Properties

Name | Type
------------ | -------------
`enabled` | boolean
`lastSync` | string
`pendingChanges` | number
`status` | string

## Example

```typescript
import type { ReplicationInfo } from '@zlayer/client'

// TODO: Update the object below with actual values
const example = {
  "enabled": null,
  "lastSync": null,
  "pendingChanges": null,
  "status": null,
} satisfies ReplicationInfo

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as ReplicationInfo
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


