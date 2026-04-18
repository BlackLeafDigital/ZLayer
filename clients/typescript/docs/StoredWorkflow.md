
# StoredWorkflow

A stored workflow — a named sequence of steps forming a DAG that composes tasks, project builds, deploys, and sync applies.

## Properties

Name | Type
------------ | -------------
`createdAt` | string
`id` | string
`name` | string
`projectId` | string
`steps` | [Array&lt;WorkflowStep&gt;](WorkflowStep.md)
`updatedAt` | string

## Example

```typescript
import type { StoredWorkflow } from '@zlayer/client'

// TODO: Update the object below with actual values
const example = {
  "createdAt": 2026-04-15T12:00:00Z,
  "id": null,
  "name": null,
  "projectId": null,
  "steps": null,
  "updatedAt": 2026-04-15T12:00:00Z,
} satisfies StoredWorkflow

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as StoredWorkflow
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


