# ContainerPortResponse

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**Ports** | [**map[string][]ContainerPortBinding**](array.md) | Map of &#x60;\&quot;&lt;port&gt;/&lt;protocol&gt;\&quot;&#x60; to host bindings. | 

## Methods

### NewContainerPortResponse

`func NewContainerPortResponse(ports map[string][]ContainerPortBinding, ) *ContainerPortResponse`

NewContainerPortResponse instantiates a new ContainerPortResponse object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewContainerPortResponseWithDefaults

`func NewContainerPortResponseWithDefaults() *ContainerPortResponse`

NewContainerPortResponseWithDefaults instantiates a new ContainerPortResponse object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetPorts

`func (o *ContainerPortResponse) GetPorts() map[string][]ContainerPortBinding`

GetPorts returns the Ports field if non-nil, zero value otherwise.

### GetPortsOk

`func (o *ContainerPortResponse) GetPortsOk() (*map[string][]ContainerPortBinding, bool)`

GetPortsOk returns a tuple with the Ports field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetPorts

`func (o *ContainerPortResponse) SetPorts(v map[string][]ContainerPortBinding)`

SetPorts sets Ports field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


