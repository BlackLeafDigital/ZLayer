
# RevealAllSecretsResponse

Response for the batch reveal endpoint — returns every secret in an env as plaintext. Admin-only for now (Phase 3 will gate this on per-env Read permission instead).

## Properties

Name | Type
------------ | -------------
`environment` | string
`secrets` | { [key: string]: string; }

## Example

```typescript
import type { RevealAllSecretsResponse } from '@zlayer/api-client'

// TODO: Update the object below with actual values
const example = {
  "environment": null,
  "secrets": null,
} satisfies RevealAllSecretsResponse

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as RevealAllSecretsResponse
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


