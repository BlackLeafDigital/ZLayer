
# ContainerExecRequest

Exec request for running a command in a container

## Properties

Name | Type
------------ | -------------
`command` | Array&lt;string&gt;

## Example

```typescript
import type { ContainerExecRequest } from '@zlayer/client'

// TODO: Update the object below with actual values
const example = {
  "command": null,
} satisfies ContainerExecRequest

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as ContainerExecRequest
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


