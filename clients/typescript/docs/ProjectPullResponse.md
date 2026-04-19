
# ProjectPullResponse

Response body for `POST /api/v1/projects/{id}/pull`.

## Properties

Name | Type
------------ | -------------
`branch` | string
`gitUrl` | string
`path` | string
`projectId` | string
`sha` | string

## Example

```typescript
import type { ProjectPullResponse } from '@zlayer/client'

// TODO: Update the object below with actual values
const example = {
  "branch": null,
  "gitUrl": null,
  "path": null,
  "projectId": null,
  "sha": null,
} satisfies ProjectPullResponse

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as ProjectPullResponse
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


