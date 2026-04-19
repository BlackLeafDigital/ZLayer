
# NodeDetails

Detailed node information

## Properties

Name | Type
------------ | -------------
`address` | string
`id` | number
`labels` | { [key: string]: string; }
`lastHeartbeat` | number
`lastSeen` | number
`registeredAt` | number
`resources` | [NodeResourceInfo](NodeResourceInfo.md)
`role` | string
`services` | Array&lt;string&gt;
`status` | string

## Example

```typescript
import type { NodeDetails } from '@zlayer/client'

// TODO: Update the object below with actual values
const example = {
  "address": null,
  "id": null,
  "labels": null,
  "lastHeartbeat": null,
  "lastSeen": null,
  "registeredAt": null,
  "resources": null,
  "role": null,
  "services": null,
  "status": null,
} satisfies NodeDetails

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as NodeDetails
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


