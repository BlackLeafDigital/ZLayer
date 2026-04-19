
# BackendsResponse

Response for `GET /api/v1/proxy/backends`.

## Properties

Name | Type
------------ | -------------
`groups` | [Array&lt;BackendGroupInfo&gt;](BackendGroupInfo.md)
`totalGroups` | number

## Example

```typescript
import type { BackendsResponse } from '@zlayer/client'

// TODO: Update the object below with actual values
const example = {
  "groups": null,
  "totalGroups": null,
} satisfies BackendsResponse

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as BackendsResponse
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


