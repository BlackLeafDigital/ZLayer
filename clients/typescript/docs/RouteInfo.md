
# RouteInfo

Information about a single registered route.

## Properties

Name | Type
------------ | -------------
`backends` | Array&lt;string&gt;
`endpoint` | string
`expose` | string
`host` | string
`pathPrefix` | string
`protocol` | string
`service` | string
`stripPrefix` | boolean
`targetPort` | number

## Example

```typescript
import type { RouteInfo } from '@zlayer/client'

// TODO: Update the object below with actual values
const example = {
  "backends": null,
  "endpoint": null,
  "expose": null,
  "host": null,
  "pathPrefix": null,
  "protocol": null,
  "service": null,
  "stripPrefix": null,
  "targetPort": null,
} satisfies RouteInfo

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as RouteInfo
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


