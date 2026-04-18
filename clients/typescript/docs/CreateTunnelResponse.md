
# CreateTunnelResponse

Response after creating a tunnel token

## Properties

Name | Type
------------ | -------------
`createdAt` | number
`expiresAt` | number
`id` | string
`name` | string
`services` | Array&lt;string&gt;
`token` | string

## Example

```typescript
import type { CreateTunnelResponse } from '@zlayer/client'

// TODO: Update the object below with actual values
const example = {
  "createdAt": null,
  "expiresAt": null,
  "id": null,
  "name": null,
  "services": null,
  "token": null,
} satisfies CreateTunnelResponse

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as CreateTunnelResponse
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


