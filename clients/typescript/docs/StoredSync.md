
# StoredSync

A stored sync resource (persistent record of a git-backed resource set).  A sync points at a directory within a project\'s checkout that contains `ZLayer` resource YAMLs. On diff/apply the directory is scanned, compared against current API state, and reconciled.

## Properties

Name | Type
------------ | -------------
`autoApply` | boolean
`createdAt` | string
`deleteMissing` | boolean
`gitPath` | string
`id` | string
`lastAppliedSha` | string
`name` | string
`projectId` | string
`updatedAt` | string

## Example

```typescript
import type { StoredSync } from '@zlayer/client'

// TODO: Update the object below with actual values
const example = {
  "autoApply": null,
  "createdAt": 2026-04-15T12:00:00Z,
  "deleteMissing": null,
  "gitPath": null,
  "id": null,
  "lastAppliedSha": null,
  "name": null,
  "projectId": null,
  "updatedAt": 2026-04-15T12:00:00Z,
} satisfies StoredSync

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as StoredSync
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


