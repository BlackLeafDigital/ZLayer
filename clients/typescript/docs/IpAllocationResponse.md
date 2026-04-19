
# IpAllocationResponse

IP allocation status response

## Properties

Name | Type
------------ | -------------
`allocatedCount` | number
`allocatedIps` | Array&lt;string&gt;
`availableCount` | number
`cidr` | string
`totalIps` | number
`utilizationPercent` | number

## Example

```typescript
import type { IpAllocationResponse } from '@zlayer/client'

// TODO: Update the object below with actual values
const example = {
  "allocatedCount": null,
  "allocatedIps": null,
  "availableCount": null,
  "cidr": null,
  "totalIps": null,
  "utilizationPercent": null,
} satisfies IpAllocationResponse

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as IpAllocationResponse
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


