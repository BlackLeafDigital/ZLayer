
# GrantPermissionRequest

Body for `POST /api/v1/permissions`.

## Properties

Name | Type
------------ | -------------
`level` | [PermissionLevel](PermissionLevel.md)
`resourceId` | string
`resourceKind` | string
`subjectId` | string
`subjectKind` | [SubjectKind](SubjectKind.md)

## Example

```typescript
import type { GrantPermissionRequest } from '@zlayer/client'

// TODO: Update the object below with actual values
const example = {
  "level": null,
  "resourceId": null,
  "resourceKind": null,
  "subjectId": null,
  "subjectKind": null,
} satisfies GrantPermissionRequest

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as GrantPermissionRequest
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


