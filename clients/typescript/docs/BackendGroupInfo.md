
# BackendGroupInfo

A load-balancer backend group for one service.

## Properties

Name | Type
------------ | -------------
`backends` | [Array&lt;BackendInfo&gt;](BackendInfo.md)
`healthyCount` | number
`service` | string
`strategy` | string
`totalCount` | number

## Example

```typescript
import type { BackendGroupInfo } from '@zlayer/client'

// TODO: Update the object below with actual values
const example = {
  "backends": null,
  "healthyCount": null,
  "service": null,
  "strategy": null,
  "totalCount": null,
} satisfies BackendGroupInfo

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as BackendGroupInfo
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


