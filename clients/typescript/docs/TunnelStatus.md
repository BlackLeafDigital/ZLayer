
# TunnelStatus

Detailed tunnel status

## Properties

Name | Type
------------ | -------------
`activeConnections` | number
`allowedServices` | Array&lt;string&gt;
`clientAddr` | string
`createdAt` | number
`expiresAt` | number
`id` | string
`lastConnected` | number
`name` | string
`registeredServices` | [Array&lt;RegisteredServiceInfo&gt;](RegisteredServiceInfo.md)
`status` | string

## Example

```typescript
import type { TunnelStatus } from '@zlayer/client'

// TODO: Update the object below with actual values
const example = {
  "activeConnections": null,
  "allowedServices": null,
  "clientAddr": null,
  "createdAt": null,
  "expiresAt": null,
  "id": null,
  "lastConnected": null,
  "name": null,
  "registeredServices": null,
  "status": null,
} satisfies TunnelStatus

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as TunnelStatus
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


