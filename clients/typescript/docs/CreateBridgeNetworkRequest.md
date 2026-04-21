
# CreateBridgeNetworkRequest

Body for `POST /api/v1/container-networks`.

## Properties

Name | Type
------------ | -------------
`driver` | [BridgeNetworkDriver](BridgeNetworkDriver.md)
`internal` | boolean
`labels` | { [key: string]: string; }
`name` | string
`subnet` | string

## Example

```typescript
import type { CreateBridgeNetworkRequest } from '@zlayer/api-client'

// TODO: Update the object below with actual values
const example = {
  "driver": null,
  "internal": null,
  "labels": null,
  "name": null,
  "subnet": null,
} satisfies CreateBridgeNetworkRequest

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as CreateBridgeNetworkRequest
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


