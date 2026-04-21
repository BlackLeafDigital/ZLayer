
# DisconnectBridgeNetworkRequest

Body for `POST /api/v1/container-networks/{id_or_name}/disconnect`.

## Properties

Name | Type
------------ | -------------
`containerId` | string
`force` | boolean

## Example

```typescript
import type { DisconnectBridgeNetworkRequest } from '@zlayer/api-client'

// TODO: Update the object below with actual values
const example = {
  "containerId": null,
  "force": null,
} satisfies DisconnectBridgeNetworkRequest

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as DisconnectBridgeNetworkRequest
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


