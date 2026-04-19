
# CreateUserRequest

Body for `POST /api/v1/users`.

## Properties

Name | Type
------------ | -------------
`displayName` | string
`email` | string
`password` | string
`role` | [UserRole](UserRole.md)

## Example

```typescript
import type { CreateUserRequest } from '@zlayer/client'

// TODO: Update the object below with actual values
const example = {
  "displayName": null,
  "email": null,
  "password": null,
  "role": null,
} satisfies CreateUserRequest

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as CreateUserRequest
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


