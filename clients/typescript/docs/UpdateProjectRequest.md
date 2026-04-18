
# UpdateProjectRequest

Body for `PATCH /api/v1/projects/{id}`. All fields are optional.

## Properties

Name | Type
------------ | -------------
`autoDeploy` | boolean
`buildKind` | [BuildKind](BuildKind.md)
`buildPath` | string
`defaultEnvironmentId` | string
`deploySpecPath` | string
`description` | string
`gitBranch` | string
`gitCredentialId` | string
`gitUrl` | string
`name` | string
`pollIntervalSecs` | number
`registryCredentialId` | string

## Example

```typescript
import type { UpdateProjectRequest } from '@zlayer/client'

// TODO: Update the object below with actual values
const example = {
  "autoDeploy": null,
  "buildKind": null,
  "buildPath": null,
  "defaultEnvironmentId": null,
  "deploySpecPath": null,
  "description": null,
  "gitBranch": null,
  "gitCredentialId": null,
  "gitUrl": null,
  "name": null,
  "pollIntervalSecs": null,
  "registryCredentialId": null,
} satisfies UpdateProjectRequest

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as UpdateProjectRequest
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


