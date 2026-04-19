
# SecretMetadataResponse

Response containing secret metadata. Never includes the value unless the caller is on the explicit `?reveal=true` admin path, in which case `value` is populated.

## Properties

Name | Type
------------ | -------------
`createdAt` | number
`name` | string
`updatedAt` | number
`value` | string
`version` | number

## Example

```typescript
import type { SecretMetadataResponse } from '@zlayer/client'

// TODO: Update the object below with actual values
const example = {
  "createdAt": null,
  "name": null,
  "updatedAt": null,
  "value": null,
  "version": null,
} satisfies SecretMetadataResponse

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as SecretMetadataResponse
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


