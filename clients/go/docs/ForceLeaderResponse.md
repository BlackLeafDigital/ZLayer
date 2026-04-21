# ForceLeaderResponse

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**Message** | **string** |  | 
**PreservedNodes** | **int32** | Number of cluster nodes whose state was preserved | 
**PreservedServices** | **int32** | Number of services whose state was preserved | 
**Success** | **bool** |  | 

## Methods

### NewForceLeaderResponse

`func NewForceLeaderResponse(message string, preservedNodes int32, preservedServices int32, success bool, ) *ForceLeaderResponse`

NewForceLeaderResponse instantiates a new ForceLeaderResponse object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewForceLeaderResponseWithDefaults

`func NewForceLeaderResponseWithDefaults() *ForceLeaderResponse`

NewForceLeaderResponseWithDefaults instantiates a new ForceLeaderResponse object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetMessage

`func (o *ForceLeaderResponse) GetMessage() string`

GetMessage returns the Message field if non-nil, zero value otherwise.

### GetMessageOk

`func (o *ForceLeaderResponse) GetMessageOk() (*string, bool)`

GetMessageOk returns a tuple with the Message field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetMessage

`func (o *ForceLeaderResponse) SetMessage(v string)`

SetMessage sets Message field to given value.


### GetPreservedNodes

`func (o *ForceLeaderResponse) GetPreservedNodes() int32`

GetPreservedNodes returns the PreservedNodes field if non-nil, zero value otherwise.

### GetPreservedNodesOk

`func (o *ForceLeaderResponse) GetPreservedNodesOk() (*int32, bool)`

GetPreservedNodesOk returns a tuple with the PreservedNodes field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetPreservedNodes

`func (o *ForceLeaderResponse) SetPreservedNodes(v int32)`

SetPreservedNodes sets PreservedNodes field to given value.


### GetPreservedServices

`func (o *ForceLeaderResponse) GetPreservedServices() int32`

GetPreservedServices returns the PreservedServices field if non-nil, zero value otherwise.

### GetPreservedServicesOk

`func (o *ForceLeaderResponse) GetPreservedServicesOk() (*int32, bool)`

GetPreservedServicesOk returns a tuple with the PreservedServices field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetPreservedServices

`func (o *ForceLeaderResponse) SetPreservedServices(v int32)`

SetPreservedServices sets PreservedServices field to given value.


### GetSuccess

`func (o *ForceLeaderResponse) GetSuccess() bool`

GetSuccess returns the Success field if non-nil, zero value otherwise.

### GetSuccessOk

`func (o *ForceLeaderResponse) GetSuccessOk() (*bool, bool)`

GetSuccessOk returns a tuple with the Success field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetSuccess

`func (o *ForceLeaderResponse) SetSuccess(v bool)`

SetSuccess sets Success field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


