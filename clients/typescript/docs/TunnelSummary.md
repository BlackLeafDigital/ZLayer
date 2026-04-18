
# TunnelSummary

Tunnel summary for list operations

## Properties

Name | Type
------------ | -------------
`createdAt` | number
`expiresAt` | number
`id` | string
`lastConnected` | number
`name` | string
`services` | Array&lt;string&gt;
`status` | string

## Example

```typescript
import type { TunnelSummary } from '@zlayer/client'

// TODO: Update the object below with actual values
const example = {
  "createdAt": null,
  "expiresAt": null,
  "id": null,
  "lastConnected": null,
  "name": null,
  "services": null,
  "status": null,
} satisfies TunnelSummary

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as TunnelSummary
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


