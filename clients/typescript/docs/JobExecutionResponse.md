
# JobExecutionResponse

Job execution status response

## Properties

Name | Type
------------ | -------------
`completedAt` | string
`durationMs` | number
`error` | string
`exitCode` | number
`id` | string
`jobName` | string
`logs` | string
`startedAt` | string
`status` | string
`trigger` | string

## Example

```typescript
import type { JobExecutionResponse } from '@zlayer/client'

// TODO: Update the object below with actual values
const example = {
  "completedAt": null,
  "durationMs": null,
  "error": null,
  "exitCode": null,
  "id": null,
  "jobName": null,
  "logs": null,
  "startedAt": null,
  "status": null,
  "trigger": null,
} satisfies JobExecutionResponse

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as JobExecutionResponse
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


