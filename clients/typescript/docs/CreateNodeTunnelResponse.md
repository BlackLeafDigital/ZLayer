
# CreateNodeTunnelResponse

Response after creating a node-to-node tunnel

## Properties

Name | Type
------------ | -------------
`expose` | string
`fromNode` | string
`localPort` | number
`name` | string
`remotePort` | number
`status` | string
`toNode` | string

## Example

```typescript
import type { CreateNodeTunnelResponse } from '@zlayer/client'

// TODO: Update the object below with actual values
const example = {
  "expose": null,
  "fromNode": null,
  "localPort": null,
  "name": null,
  "remotePort": null,
  "status": null,
  "toNode": null,
} satisfies CreateNodeTunnelResponse

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as CreateNodeTunnelResponse
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


