
# WorkflowRun

A recorded execution of a workflow.

## Properties

Name | Type
------------ | -------------
`finishedAt` | string
`id` | string
`startedAt` | string
`status` | [WorkflowRunStatus](WorkflowRunStatus.md)
`stepResults` | [Array&lt;StepResult&gt;](StepResult.md)
`workflowId` | string

## Example

```typescript
import type { WorkflowRun } from '@zlayer/client'

// TODO: Update the object below with actual values
const example = {
  "finishedAt": 2026-04-15T12:00:01Z,
  "id": null,
  "startedAt": 2026-04-15T12:00:00Z,
  "status": null,
  "stepResults": null,
  "workflowId": null,
} satisfies WorkflowRun

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as WorkflowRun
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


