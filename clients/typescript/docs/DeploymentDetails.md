
# DeploymentDetails

Deployment details (enhanced with per-service health and endpoints)

## Properties

Name | Type
------------ | -------------
`createdAt` | string
`name` | string
`serviceHealth` | [Array&lt;ServiceHealthInfo&gt;](ServiceHealthInfo.md)
`services` | Array&lt;string&gt;
`status` | string
`updatedAt` | string

## Example

```typescript
import type { DeploymentDetails } from '@zlayer/client'

// TODO: Update the object below with actual values
const example = {
  "createdAt": null,
  "name": null,
  "serviceHealth": null,
  "services": null,
  "status": null,
  "updatedAt": null,
} satisfies DeploymentDetails

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as DeploymentDetails
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


