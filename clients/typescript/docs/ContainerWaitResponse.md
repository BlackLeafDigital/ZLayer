
# ContainerWaitResponse

Wait response with container exit code plus optional classification fields (added in §3.12 of the SDK-fixes spec).  The three optional fields (`reason`, `signal`, `finished_at`) are additive — clients that only read `exit_code` keep working unchanged.

## Properties

Name | Type
------------ | -------------
`exitCode` | number
`finishedAt` | string
`id` | string
`reason` | string
`signal` | string

## Example

```typescript
import type { ContainerWaitResponse } from '@zlayer/api-client'

// TODO: Update the object below with actual values
const example = {
  "exitCode": null,
  "finishedAt": null,
  "id": null,
  "reason": null,
  "signal": null,
} satisfies ContainerWaitResponse

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as ContainerWaitResponse
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


