
# StoredProject

A project bundles a git source, build configuration, registry credential reference, linked deployments, and a default environment.

## Properties

Name | Type
------------ | -------------
`autoDeploy` | boolean
`buildKind` | [BuildKind](BuildKind.md)
`buildPath` | string
`createdAt` | string
`defaultEnvironmentId` | string
`deploySpecPath` | string
`description` | string
`gitBranch` | string
`gitCredentialId` | string
`gitUrl` | string
`id` | string
`name` | string
`ownerId` | string
`pollIntervalSecs` | number
`registryCredentialId` | string
`updatedAt` | string

## Example

```typescript
import type { StoredProject } from '@zlayer/client'

// TODO: Update the object below with actual values
const example = {
  "autoDeploy": null,
  "buildKind": null,
  "buildPath": null,
  "createdAt": 2026-04-15T12:00:00Z,
  "defaultEnvironmentId": null,
  "deploySpecPath": null,
  "description": null,
  "gitBranch": null,
  "gitCredentialId": null,
  "gitUrl": null,
  "id": null,
  "name": null,
  "ownerId": null,
  "pollIntervalSecs": null,
  "registryCredentialId": null,
  "updatedAt": 2026-04-15T12:00:00Z,
} satisfies StoredProject

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as StoredProject
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


