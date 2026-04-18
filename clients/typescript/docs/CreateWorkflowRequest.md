
# CreateWorkflowRequest

Body for `POST /api/v1/workflows`.

## Properties

Name | Type
------------ | -------------
`name` | string
`projectId` | string
`steps` | [Array&lt;WorkflowStep&gt;](WorkflowStep.md)

## Example

```typescript
import type { CreateWorkflowRequest } from '@zlayer/client'

// TODO: Update the object below with actual values
const example = {
  "name": null,
  "projectId": null,
  "steps": null,
} satisfies CreateWorkflowRequest

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as CreateWorkflowRequest
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


