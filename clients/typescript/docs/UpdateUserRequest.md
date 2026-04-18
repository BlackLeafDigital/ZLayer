
# UpdateUserRequest

Body for `PATCH /api/v1/users/{id}`. All fields are optional so a caller can update a single attribute without touching the others.

## Properties

Name | Type
------------ | -------------
`displayName` | string
`isActive` | boolean
`role` | [UserRole](UserRole.md)

## Example

```typescript
import type { UpdateUserRequest } from '@zlayer/client'

// TODO: Update the object below with actual values
const example = {
  "displayName": null,
  "isActive": null,
  "role": null,
} satisfies UpdateUserRequest

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as UpdateUserRequest
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


