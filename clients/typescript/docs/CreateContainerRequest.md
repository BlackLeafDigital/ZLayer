
# CreateContainerRequest

Request to create and start a container

## Properties

Name | Type
------------ | -------------
`command` | Array&lt;string&gt;
`dns` | Array&lt;string&gt;
`env` | { [key: string]: string; }
`extraHosts` | Array&lt;string&gt;
`healthCheck` | [HealthCheckRequest](HealthCheckRequest.md)
`hostname` | string
`image` | string
`labels` | { [key: string]: string; }
`name` | string
`networks` | [Array&lt;NetworkAttachmentRequest&gt;](NetworkAttachmentRequest.md)
`ports` | [Array&lt;PortMapping&gt;](PortMapping.md)
`pullPolicy` | string
`registryAuth` | [RegistryAuth](RegistryAuth.md)
`registryCredentialId` | string
`resources` | [ContainerResourceLimits](ContainerResourceLimits.md)
`restartPolicy` | [ContainerRestartPolicy](ContainerRestartPolicy.md)
`volumes` | [Array&lt;VolumeMount&gt;](VolumeMount.md)
`workDir` | string

## Example

```typescript
import type { CreateContainerRequest } from '@zlayer/api-client'

// TODO: Update the object below with actual values
const example = {
  "command": null,
  "dns": null,
  "env": null,
  "extraHosts": null,
  "healthCheck": null,
  "hostname": null,
  "image": null,
  "labels": null,
  "name": null,
  "networks": null,
  "ports": null,
  "pullPolicy": null,
  "registryAuth": null,
  "registryCredentialId": null,
  "resources": null,
  "restartPolicy": null,
  "volumes": null,
  "workDir": null,
} satisfies CreateContainerRequest

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as CreateContainerRequest
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


