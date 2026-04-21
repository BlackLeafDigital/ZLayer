
# OidcProviderPublic

Public-facing projection — what the Manager UI needs to render a login button. Never includes the client secret.

## Properties

Name | Type
------------ | -------------
`displayName` | string
`name` | string

## Example

```typescript
import type { OidcProviderPublic } from '@zlayer/api-client'

// TODO: Update the object below with actual values
const example = {
  "displayName": null,
  "name": null,
} satisfies OidcProviderPublic

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as OidcProviderPublic
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


