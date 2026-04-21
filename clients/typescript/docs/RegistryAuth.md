
# RegistryAuth

Inline Docker/OCI registry credentials attached to a single pull request.  Prefer persistent credentials via `/api/v1/credentials/registry` for long-lived services. Use this inline form for one-off pulls (e.g. CI runners fetching a private image for a single job) where persisting a credential is undesirable.

## Properties

Name | Type
------------ | -------------
`authType` | [RegistryAuthType](RegistryAuthType.md)
`password` | string
`username` | string

## Example

```typescript
import type { RegistryAuth } from '@zlayer/api-client'

// TODO: Update the object below with actual values
const example = {
  "authType": null,
  "password": null,
  "username": null,
} satisfies RegistryAuth

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as RegistryAuth
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


