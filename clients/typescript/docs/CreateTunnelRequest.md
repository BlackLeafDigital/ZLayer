
# CreateTunnelRequest

Request to create a new tunnel token

## Properties

Name | Type
------------ | -------------
`name` | string
`services` | Array&lt;string&gt;
`ttlSecs` | number

## Example

```typescript
import type { CreateTunnelRequest } from '@zlayer/client'

// TODO: Update the object below with actual values
const example = {
  "name": null,
  "services": null,
  "ttlSecs": null,
} satisfies CreateTunnelRequest

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as CreateTunnelRequest
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


