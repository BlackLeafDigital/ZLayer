
# NodeResourceInfo

Node resource information

## Properties

Name | Type
------------ | -------------
`cpuPercent` | number
`cpuTotal` | number
`cpuUsed` | number
`memoryPercent` | number
`memoryTotal` | number
`memoryUsed` | number

## Example

```typescript
import type { NodeResourceInfo } from '@zlayer/client'

// TODO: Update the object below with actual values
const example = {
  "cpuPercent": null,
  "cpuTotal": null,
  "cpuUsed": null,
  "memoryPercent": null,
  "memoryTotal": null,
  "memoryUsed": null,
} satisfies NodeResourceInfo

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as NodeResourceInfo
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


