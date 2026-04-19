
# ContainerStatsResponse

Container resource statistics

## Properties

Name | Type
------------ | -------------
`cpuUsageUsec` | number
`id` | string
`memoryBytes` | number
`memoryLimit` | number
`memoryPercent` | number

## Example

```typescript
import type { ContainerStatsResponse } from '@zlayer/client'

// TODO: Update the object below with actual values
const example = {
  "cpuUsageUsec": null,
  "id": null,
  "memoryBytes": null,
  "memoryLimit": null,
  "memoryPercent": null,
} satisfies ContainerStatsResponse

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as ContainerStatsResponse
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


