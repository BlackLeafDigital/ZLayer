
# PortMapping

A single host-to-container port publish rule (Docker\'s `-p`).  When `host_port` is `None` (or explicitly `Some(0)`), the container runtime assigns an ephemeral host port. `host_ip` defaults to `\"0.0.0.0\"` to bind on all interfaces.

## Properties

Name | Type
------------ | -------------
`containerPort` | number
`hostIp` | string
`hostPort` | number
`protocol` | [PortProtocol](PortProtocol.md)

## Example

```typescript
import type { PortMapping } from '@zlayer/api-client'

// TODO: Update the object below with actual values
const example = {
  "containerPort": null,
  "hostIp": null,
  "hostPort": null,
  "protocol": null,
} satisfies PortMapping

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as PortMapping
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


