
# BuildStatus

Build status response

## Properties

Name | Type
------------ | -------------
`completedAt` | string
`error` | string
`id` | string
`imageId` | string
`startedAt` | string
`status` | [BuildStateEnum](BuildStateEnum.md)

## Example

```typescript
import type { BuildStatus } from '@zlayer/client'

// TODO: Update the object below with actual values
const example = {
  "completedAt": null,
  "error": null,
  "id": null,
  "imageId": null,
  "startedAt": null,
  "status": null,
} satisfies BuildStatus

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as BuildStatus
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


