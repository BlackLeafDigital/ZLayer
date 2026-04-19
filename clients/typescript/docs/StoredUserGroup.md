
# StoredUserGroup

A stored user group for role-based access control.

## Properties

Name | Type
------------ | -------------
`createdAt` | string
`description` | string
`id` | string
`name` | string
`updatedAt` | string

## Example

```typescript
import type { StoredUserGroup } from '@zlayer/client'

// TODO: Update the object below with actual values
const example = {
  "createdAt": 2026-04-15T12:00:00Z,
  "description": null,
  "id": null,
  "name": null,
  "updatedAt": 2026-04-15T12:00:00Z,
} satisfies StoredUserGroup

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as StoredUserGroup
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


