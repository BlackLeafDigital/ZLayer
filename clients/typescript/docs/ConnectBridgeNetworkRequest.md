
# ConnectBridgeNetworkRequest

Body for `POST /api/v1/container-networks/{id_or_name}/connect`.

## Properties

Name | Type
------------ | -------------
`aliases` | Array&lt;string&gt;
`containerId` | string
`ipv4Address` | string

## Example

```typescript
import type { ConnectBridgeNetworkRequest } from '@zlayer/api-client'

// TODO: Update the object below with actual values
const example = {
  "aliases": null,
  "containerId": null,
  "ipv4Address": null,
} satisfies ConnectBridgeNetworkRequest

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as ConnectBridgeNetworkRequest
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


