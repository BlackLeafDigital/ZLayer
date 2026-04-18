
# GpuInfoSummary

Summary of a GPU on a node, stored in Raft cluster state

## Properties

Name | Type
------------ | -------------
`memoryMb` | number
`model` | string
`vendor` | string

## Example

```typescript
import type { GpuInfoSummary } from '@zlayer/client'

// TODO: Update the object below with actual values
const example = {
  "memoryMb": null,
  "model": null,
  "vendor": null,
} satisfies GpuInfoSummary

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as GpuInfoSummary
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


