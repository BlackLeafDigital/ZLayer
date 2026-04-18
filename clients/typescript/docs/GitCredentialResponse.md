
# GitCredentialResponse

Git credential metadata (returned by list/create; no secret value).

## Properties

Name | Type
------------ | -------------
`id` | string
`kind` | [GitCredentialKindSchema](GitCredentialKindSchema.md)
`name` | string

## Example

```typescript
import type { GitCredentialResponse } from '@zlayer/client'

// TODO: Update the object below with actual values
const example = {
  "id": null,
  "kind": null,
  "name": null,
} satisfies GitCredentialResponse

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as GitCredentialResponse
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


