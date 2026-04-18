
# ServiceHealthInfo

Per-service health info included in deployment details

## Properties

Name | Type
------------ | -------------
`endpoints` | Array&lt;string&gt;
`health` | string
`name` | string
`replicasDesired` | number
`replicasRunning` | number

## Example

```typescript
import type { ServiceHealthInfo } from '@zlayer/client'

// TODO: Update the object below with actual values
const example = {
  "endpoints": null,
  "health": null,
  "name": null,
  "replicasDesired": null,
  "replicasRunning": null,
} satisfies ServiceHealthInfo

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as ServiceHealthInfo
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


