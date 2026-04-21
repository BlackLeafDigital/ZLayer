
# BridgeNetworkDetails

Response body for `GET /api/v1/container-networks/{id_or_name}`.

## Properties

Name | Type
------------ | -------------
`createdAt` | Date
`driver` | [BridgeNetworkDriver](BridgeNetworkDriver.md)
`id` | string
`internal` | boolean
`labels` | { [key: string]: string; }
`name` | string
`subnet` | string
`attachedContainers` | [Array&lt;BridgeNetworkAttachment&gt;](BridgeNetworkAttachment.md)

## Example

```typescript
import type { BridgeNetworkDetails } from '@zlayer/api-client'

// TODO: Update the object below with actual values
const example = {
  "createdAt": null,
  "driver": null,
  "id": null,
  "internal": null,
  "labels": null,
  "name": null,
  "subnet": null,
  "attachedContainers": null,
} satisfies BridgeNetworkDetails

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as BridgeNetworkDetails
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


