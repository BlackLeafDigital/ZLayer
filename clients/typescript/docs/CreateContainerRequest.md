
# CreateContainerRequest

Request to create and start a container

## Properties

Name | Type
------------ | -------------
`command` | Array&lt;string&gt;
`env` | { [key: string]: string; }
`image` | string
`labels` | { [key: string]: string; }
`name` | string
`pullPolicy` | string
`resources` | [ContainerResourceLimits](ContainerResourceLimits.md)
`volumes` | [Array&lt;VolumeMount&gt;](VolumeMount.md)
`workDir` | string

## Example

```typescript
import type { CreateContainerRequest } from '@zlayer/client'

// TODO: Update the object below with actual values
const example = {
  "command": null,
  "env": null,
  "image": null,
  "labels": null,
  "name": null,
  "pullPolicy": null,
  "resources": null,
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


