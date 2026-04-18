
# BuildRequestWithContext

Build request with server-side context path

## Properties

Name | Type
------------ | -------------
`buildArgs` | { [key: string]: string; }
`contextPath` | string
`noCache` | boolean
`push` | boolean
`runtime` | string
`tags` | Array&lt;string&gt;
`target` | string

## Example

```typescript
import type { BuildRequestWithContext } from '@zlayer/client'

// TODO: Update the object below with actual values
const example = {
  "buildArgs": null,
  "contextPath": null,
  "noCache": null,
  "push": null,
  "runtime": null,
  "tags": null,
  "target": null,
} satisfies BuildRequestWithContext

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as BuildRequestWithContext
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


