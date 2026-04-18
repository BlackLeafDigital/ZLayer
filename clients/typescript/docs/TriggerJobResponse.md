
# TriggerJobResponse

Response after triggering a job

## Properties

Name | Type
------------ | -------------
`executionId` | string
`message` | string

## Example

```typescript
import type { TriggerJobResponse } from '@zlayer/client'

// TODO: Update the object below with actual values
const example = {
  "executionId": null,
  "message": null,
} satisfies TriggerJobResponse

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as TriggerJobResponse
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


