
# UserView

Response shape used by login + bootstrap + me.

## Properties

Name | Type
------------ | -------------
`displayName` | string
`email` | string
`id` | string
`isActive` | boolean
`lastLoginAt` | string
`role` | string

## Example

```typescript
import type { UserView } from '@zlayer/client'

// TODO: Update the object below with actual values
const example = {
  "displayName": null,
  "email": null,
  "id": null,
  "isActive": null,
  "lastLoginAt": 2026-04-15T12:00:00Z,
  "role": null,
} satisfies UserView

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as UserView
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


