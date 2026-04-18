
# ServiceMetrics

Service metrics

## Properties

Name | Type
------------ | -------------
`cpuPercent` | number
`memoryPercent` | number
`rps` | number

## Example

```typescript
import type { ServiceMetrics } from '@zlayer/client'

// TODO: Update the object below with actual values
const example = {
  "cpuPercent": null,
  "memoryPercent": null,
  "rps": null,
} satisfies ServiceMetrics

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as ServiceMetrics
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


