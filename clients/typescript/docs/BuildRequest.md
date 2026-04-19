
# BuildRequest

Build request for JSON API

## Properties

Name | Type
------------ | -------------
`buildArgs` | { [key: string]: string; }
`noCache` | boolean
`push` | boolean
`runtime` | string
`tags` | Array&lt;string&gt;
`target` | string

## Example

```typescript
import type { BuildRequest } from '@zlayer/client'

// TODO: Update the object below with actual values
const example = {
  "buildArgs": null,
  "noCache": null,
  "push": null,
  "runtime": null,
  "tags": null,
  "target": null,
} satisfies BuildRequest

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as BuildRequest
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


