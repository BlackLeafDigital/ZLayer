
# CreateRegistryCredentialRequest

Body for `POST /api/v1/credentials/registry`.

## Properties

Name | Type
------------ | -------------
`authType` | [RegistryAuthTypeSchema](RegistryAuthTypeSchema.md)
`password` | string
`registry` | string
`username` | string

## Example

```typescript
import type { CreateRegistryCredentialRequest } from '@zlayer/client'

// TODO: Update the object below with actual values
const example = {
  "authType": null,
  "password": null,
  "registry": null,
  "username": null,
} satisfies CreateRegistryCredentialRequest

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as CreateRegistryCredentialRequest
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


