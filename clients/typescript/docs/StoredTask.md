
# StoredTask

A stored task — a named runnable script that can be executed on demand.  Tasks can be global (`project_id = None`) or project-scoped (`project_id = Some(project_id)`).

## Properties

Name | Type
------------ | -------------
`body` | string
`createdAt` | string
`id` | string
`kind` | [TaskKind](TaskKind.md)
`name` | string
`projectId` | string
`updatedAt` | string

## Example

```typescript
import type { StoredTask } from '@zlayer/client'

// TODO: Update the object below with actual values
const example = {
  "body": null,
  "createdAt": 2026-04-15T12:00:00Z,
  "id": null,
  "kind": null,
  "name": null,
  "projectId": null,
  "updatedAt": 2026-04-15T12:00:00Z,
} satisfies StoredTask

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as StoredTask
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


