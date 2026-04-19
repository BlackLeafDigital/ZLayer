
# StoredPermission

A stored permission grant binding a subject (user or group) to a resource with a specific access level.

## Properties

Name | Type
------------ | -------------
`createdAt` | string
`id` | string
`level` | [PermissionLevel](PermissionLevel.md)
`resourceId` | string
`resourceKind` | string
`subjectId` | string
`subjectKind` | [SubjectKind](SubjectKind.md)

## Example

```typescript
import type { StoredPermission } from '@zlayer/client'

// TODO: Update the object below with actual values
const example = {
  "createdAt": 2026-04-15T12:00:00Z,
  "id": null,
  "level": null,
  "resourceId": null,
  "resourceKind": null,
  "subjectId": null,
  "subjectKind": null,
} satisfies StoredPermission

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as StoredPermission
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


