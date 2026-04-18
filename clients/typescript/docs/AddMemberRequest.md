
# AddMemberRequest

Body for `POST /api/v1/groups/{id}/members`.

## Properties

Name | Type
------------ | -------------
`userId` | string

## Example

```typescript
import type { AddMemberRequest } from '@zlayer/client'

// TODO: Update the object below with actual values
const example = {
  "userId": null,
} satisfies AddMemberRequest

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as AddMemberRequest
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


