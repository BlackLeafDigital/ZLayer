
# TaskRun

A recorded execution of a task.

## Properties

Name | Type
------------ | -------------
`exitCode` | number
`finishedAt` | string
`id` | string
`startedAt` | string
`stderr` | string
`stdout` | string
`taskId` | string

## Example

```typescript
import type { TaskRun } from '@zlayer/client'

// TODO: Update the object below with actual values
const example = {
  "exitCode": null,
  "finishedAt": 2026-04-15T12:00:01Z,
  "id": null,
  "startedAt": 2026-04-15T12:00:00Z,
  "stderr": null,
  "stdout": null,
  "taskId": null,
} satisfies TaskRun

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as TaskRun
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


