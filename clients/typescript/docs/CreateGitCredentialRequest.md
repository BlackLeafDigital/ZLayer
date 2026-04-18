
# CreateGitCredentialRequest

Body for `POST /api/v1/credentials/git`.

## Properties

Name | Type
------------ | -------------
`kind` | [GitCredentialKindSchema](GitCredentialKindSchema.md)
`name` | string
`value` | string

## Example

```typescript
import type { CreateGitCredentialRequest } from '@zlayer/client'

// TODO: Update the object below with actual values
const example = {
  "kind": null,
  "name": null,
  "value": null,
} satisfies CreateGitCredentialRequest

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as CreateGitCredentialRequest
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


