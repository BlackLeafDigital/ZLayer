
# GpuUtilizationReport

Per-GPU utilization snapshot reported in node heartbeats

## Properties

Name | Type
------------ | -------------
`index` | number
`memoryTotalMb` | number
`memoryUsedMb` | number
`powerDrawW` | number
`temperatureC` | number
`utilizationPercent` | number

## Example

```typescript
import type { GpuUtilizationReport } from '@zlayer/client'

// TODO: Update the object below with actual values
const example = {
  "index": null,
  "memoryTotalMb": null,
  "memoryUsedMb": null,
  "powerDrawW": null,
  "temperatureC": null,
  "utilizationPercent": null,
} satisfies GpuUtilizationReport

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as GpuUtilizationReport
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


